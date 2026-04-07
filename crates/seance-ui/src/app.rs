use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::Receiver,
};

use anyhow::Result;
use gpui::{
    AnyWindowHandle, App, AppContext, Application, BorrowAppContext, Bounds, Context, Global,
    KeyBinding, Window, WindowBackgroundAppearance, WindowBounds, WindowOptions, px, size,
};
use seance_core::{AppControllerHandle, PlatformCloseAction, WindowTarget};
use seance_terminal::TerminalGeometry;
use tracing::trace;

use crate::{
    CheckForUpdates, CloseActiveSession, ConnectHost, ConnectHostInNewWindow, HideOtherApps,
    HideSeance, NewTerminal, OpenCommandPalette, OpenNewWindow, OpenPreferences, QuitSeance,
    SeanceWorkspace, SelectSession, SettingsSection, ShowAllApps, SwitchTheme, TogglePerfHud,
    backend::UiBackend,
    connect::ConnectAttemptTracker,
    forms::{SecureWorkspaceState, SettingsPanelState, VaultModalState, WorkspaceSurface},
    perf::{PerfOverlayState, RedrawReason, perf_mode_from_config, perf_mode_override_from_env},
    surface::TerminalSurfaceState,
    ui_components::theme_id_from_config,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiCommand {
    OpenWindow { target: WindowTarget },
    ActivateApp,
    HideApp,
    QuitApp,
    OpenHost { vault_id: String, host_id: String },
}

#[derive(Default)]
pub struct UiIntegration {
    pub configure_application: Option<Box<dyn Fn(&Application)>>,
    pub configure_app: Option<Box<dyn Fn(&mut App)>>,
    pub refresh_app_menus: Option<Arc<dyn Fn(&mut App) + Send + Sync>>,
}

pub struct UiRuntime {
    pub controller: AppControllerHandle,
    pub commands: Receiver<UiCommand>,
    pub integration: UiIntegration,
}

pub fn run(runtime: UiRuntime) -> Result<()> {
    let UiRuntime {
        controller,
        commands,
        mut integration,
    } = runtime;
    let backend = UiBackend::new(controller)?;
    let menu_refresher = integration.refresh_app_menus.clone();
    let application = Application::new();
    if let Some(configure_application) = integration.configure_application.take() {
        configure_application(&application);
    }

    application.run(move |cx: &mut App| {
        if let Some(menu_refresher) = menu_refresher.clone() {
            cx.set_global(AppMenuRefresher(menu_refresher));
        }
        cx.set_global(WorkspaceWindowRegistry::default());
        register_app_actions(cx, backend.clone());
        if let Some(configure_app) = integration.configure_app.take() {
            configure_app(cx);
        }
        refresh_app_menus(cx);

        let async_app = cx.to_async();
        let quit_requested = Arc::new(AtomicBool::new(false));
        let backend_for_close = backend.clone();
        let quit_requested_for_close = Arc::clone(&quit_requested);
        cx.on_window_closed(move |cx| {
            backend_for_close.controller().on_window_closed();
            WorkspaceWindowRegistry::retain_open_windows(cx);
            if cx.windows().is_empty() && !quit_requested_for_close.load(Ordering::Relaxed) {
                match backend_for_close.controller().on_last_window_closed() {
                    PlatformCloseAction::Hide => cx.hide(),
                    PlatformCloseAction::Exit => cx.quit(),
                }
            }
            refresh_app_menus(cx);
        })
        .detach();

        let commands = Arc::new(Mutex::new(commands));
        let backend_for_commands = backend.clone();
        let quit_requested_for_commands = Arc::clone(&quit_requested);
        cx.foreground_executor()
            .spawn(async move {
                loop {
                    let commands_for_recv = Arc::clone(&commands);
                    let recv_result = async_app
                        .background_executor()
                        .spawn(async move { commands_for_recv.lock().unwrap().recv() })
                        .await;
                    let Ok(command) = recv_result else {
                        break;
                    };
                    let backend = backend_for_commands.clone();
                    let quit_requested = Arc::clone(&quit_requested_for_commands);
                    let _ = async_app.update(move |cx| match command {
                        UiCommand::OpenWindow { target } => {
                            let _ = open_workspace_window(cx, backend, target, None);
                            refresh_app_menus(cx);
                        }
                        UiCommand::ActivateApp => cx.activate(false),
                        UiCommand::HideApp => cx.hide(),
                        UiCommand::QuitApp => {
                            quit_requested.store(true, Ordering::Relaxed);
                            cx.quit();
                        }
                        UiCommand::OpenHost { vault_id, host_id } => {
                            let _ = open_workspace_window(
                                cx,
                                backend,
                                WindowTarget::MostRecentOrNew,
                                Some(InitialWorkspaceAction::ConnectHost { vault_id, host_id }),
                            );
                            cx.activate(false);
                            refresh_app_menus(cx);
                        }
                    });
                }
            })
            .detach();
    });
    Ok(())
}

#[derive(Clone)]
struct AppMenuRefresher(Arc<dyn Fn(&mut App) + Send + Sync>);

impl Global for AppMenuRefresher {}

#[derive(Default)]
struct WorkspaceWindowRegistry {
    ordered: Vec<AnyWindowHandle>,
}

impl Global for WorkspaceWindowRegistry {}

impl WorkspaceWindowRegistry {
    fn ordered_handles(&self) -> Vec<AnyWindowHandle> {
        self.ordered.clone()
    }

    fn register(cx: &mut App, handle: AnyWindowHandle) {
        cx.update_global(|registry: &mut Self, _| {
            promote_unique(&mut registry.ordered, handle);
        });
    }

    fn unregister(cx: &mut App, handle: AnyWindowHandle) {
        cx.update_global(|registry: &mut Self, _| {
            remove_item(&mut registry.ordered, handle);
        });
    }

    fn promote(cx: &mut App, handle: AnyWindowHandle) {
        cx.update_global(|registry: &mut Self, _| {
            promote_unique(&mut registry.ordered, handle);
        });
    }

    fn retain_open_windows(cx: &mut App) {
        let live_windows = cx.windows();
        cx.update_global(|registry: &mut Self, _| {
            registry
                .ordered
                .retain(|handle| live_windows.contains(handle));
        });
    }
}

#[derive(Clone, Debug)]
pub(crate) enum InitialWorkspaceAction {
    ConnectHost { vault_id: String, host_id: String },
    CheckForUpdates,
    OpenPreferences,
    OpenCommandPalette,
    TogglePerfHud,
}

pub(crate) fn refresh_app_menus(cx: &mut App) {
    let refresher = cx.try_global::<AppMenuRefresher>().cloned();
    if let Some(refresher) = refresher {
        (refresher.0)(cx);
    }
}

pub(crate) fn promote_unique<H: Copy + PartialEq>(ordered: &mut Vec<H>, item: H) {
    remove_item(ordered, item);
    ordered.insert(0, item);
}

pub(crate) fn remove_item<H: PartialEq>(ordered: &mut Vec<H>, item: H) {
    if let Some(index) = ordered.iter().position(|existing| *existing == item) {
        ordered.remove(index);
    }
}

fn push_unique_handle(handles: &mut Vec<AnyWindowHandle>, handle: AnyWindowHandle) {
    if !handles.contains(&handle) {
        handles.push(handle);
    }
}

fn workspace_window_candidates(cx: &App) -> Vec<AnyWindowHandle> {
    let active_window = cx.active_window();
    let window_stack = cx.window_stack().unwrap_or_default();
    let registered = cx
        .try_global::<WorkspaceWindowRegistry>()
        .map(WorkspaceWindowRegistry::ordered_handles)
        .unwrap_or_default();
    let live_windows = cx.windows();

    let mut candidates = Vec::new();
    if let Some(active_window) = active_window {
        push_unique_handle(&mut candidates, active_window);
    }
    for window_handle in window_stack {
        push_unique_handle(&mut candidates, window_handle);
    }
    for window_handle in registered {
        push_unique_handle(&mut candidates, window_handle);
    }
    for window_handle in live_windows {
        push_unique_handle(&mut candidates, window_handle);
    }
    candidates
}

fn with_registered_workspace(
    cx: &mut App,
    mut update: impl FnMut(&mut SeanceWorkspace, &mut Window, &mut Context<SeanceWorkspace>),
) -> bool {
    let candidates = workspace_window_candidates(cx);

    if candidates.is_empty() {
        trace!("with_registered_workspace: no candidate windows in registry");
    }

    let mut stale_handles = Vec::new();

    for window_handle in candidates {
        match window_handle.update(cx, |root, window, cx| {
            let Ok(workspace) = root.downcast::<SeanceWorkspace>() else {
                trace!(
                    "with_registered_workspace: downcast failed for window {:?}",
                    window_handle.window_id()
                );
                return false;
            };
            cx.activate(false);
            window.activate_window();
            workspace.update(cx, |this, cx| {
                update(this, window, cx);
            });
            true
        }) {
            Ok(true) => {
                WorkspaceWindowRegistry::promote(cx, window_handle);
                return true;
            }
            Ok(false) => stale_handles.push(window_handle),
            Err(err) => {
                trace!(
                    "with_registered_workspace: window update failed for {:?}: {err}",
                    window_handle.window_id()
                );
                stale_handles.push(window_handle);
            }
        }
    }

    for handle in stale_handles {
        WorkspaceWindowRegistry::unregister(cx, handle);
    }

    false
}

fn register_app_actions(cx: &mut App, backend: UiBackend) {
    cx.bind_keys([
        KeyBinding::new("cmd-,", OpenPreferences, None),
        KeyBinding::new("cmd-k", OpenCommandPalette, None),
        KeyBinding::new("cmd-t", NewTerminal, None),
        KeyBinding::new("cmd-w", CloseActiveSession, None),
        KeyBinding::new("cmd-shift-.", TogglePerfHud, None),
        KeyBinding::new("cmd-n", OpenNewWindow, None),
        KeyBinding::new("cmd-q", QuitSeance, None),
        KeyBinding::new("cmd-h", HideSeance, None),
    ]);

    let backend_for_new_terminal = backend.clone();
    cx.on_action(move |_: &NewTerminal, cx| {
        if !with_registered_workspace(cx, |this, window, cx| this.spawn_session(window, cx)) {
            let _ = open_workspace_window(
                cx,
                backend_for_new_terminal.clone(),
                WindowTarget::NewLocal,
                None,
            );
        }
        refresh_app_menus(cx);
    });

    let backend_for_updates = backend.clone();
    cx.on_action(move |_: &CheckForUpdates, cx| {
        if !with_registered_workspace(cx, |this, _window, cx| this.check_for_updates(cx)) {
            let _ = open_workspace_window(
                cx,
                backend_for_updates.clone(),
                WindowTarget::MostRecentOrNew,
                Some(InitialWorkspaceAction::CheckForUpdates),
            );
        }
        refresh_app_menus(cx);
    });

    let backend_for_palette = backend.clone();
    cx.on_action(move |_: &OpenCommandPalette, cx| {
        if !with_registered_workspace(cx, |this, _window, cx| this.toggle_palette(cx)) {
            let _ = open_workspace_window(
                cx,
                backend_for_palette.clone(),
                WindowTarget::MostRecentOrNew,
                Some(InitialWorkspaceAction::OpenCommandPalette),
            );
        }
    });

    let backend_for_preferences = backend.clone();
    cx.on_action(move |_: &OpenPreferences, cx| {
        if !with_registered_workspace(cx, |this, _window, cx| {
            this.open_settings_panel(SettingsSection::General, cx)
        }) {
            let _ = open_workspace_window(
                cx,
                backend_for_preferences.clone(),
                WindowTarget::MostRecentOrNew,
                Some(InitialWorkspaceAction::OpenPreferences),
            );
        }
        refresh_app_menus(cx);
    });

    cx.on_action(move |_: &CloseActiveSession, cx| {
        let handled = with_registered_workspace(cx, |this, _window, cx| {
            if this.active_session_id != 0 {
                this.close_session(this.active_session_id, cx);
            }
        });
        if handled {
            refresh_app_menus(cx);
        }
    });

    let backend_for_new_window = backend.clone();
    cx.on_action(move |_: &OpenNewWindow, cx| {
        let _ = open_workspace_window(
            cx,
            backend_for_new_window.clone(),
            WindowTarget::MostRecentOrNew,
            None,
        );
        cx.activate(false);
        refresh_app_menus(cx);
    });

    let backend_for_toggle_perf = backend.clone();
    cx.on_action(move |_: &TogglePerfHud, cx| {
        if !with_registered_workspace(cx, |this, window, cx| this.toggle_perf_mode(window, cx)) {
            let _ = open_workspace_window(
                cx,
                backend_for_toggle_perf.clone(),
                WindowTarget::MostRecentOrNew,
                Some(InitialWorkspaceAction::TogglePerfHud),
            );
        }
    });

    cx.on_action(move |_: &QuitSeance, cx| {
        cx.quit();
    });

    cx.on_action(move |_: &HideSeance, cx| {
        cx.hide();
    });

    cx.on_action(move |_: &HideOtherApps, _cx| {});
    cx.on_action(move |_: &ShowAllApps, _cx| {});

    let backend_for_connect_host = backend.clone();
    cx.on_action(move |action: &ConnectHost, cx| {
        let vault_id = action.vault_id.clone();
        let host_id = action.host_id.clone();
        if !with_registered_workspace(cx, |this, window, cx| {
            this.selected_host_id = Some(crate::workspace::host_scope_key(&vault_id, &host_id));
            this.start_connect_attempt(&vault_id, &host_id, window, cx);
        }) {
            let _ = open_workspace_window(
                cx,
                backend_for_connect_host.clone(),
                WindowTarget::MostRecentOrNew,
                Some(InitialWorkspaceAction::ConnectHost { vault_id, host_id }),
            );
        }
        refresh_app_menus(cx);
    });

    let backend_for_connect_host_new_window = backend.clone();
    cx.on_action(move |action: &ConnectHostInNewWindow, cx| {
        let _ = open_workspace_window(
            cx,
            backend_for_connect_host_new_window.clone(),
            WindowTarget::MostRecentOrNew,
            Some(InitialWorkspaceAction::ConnectHost {
                vault_id: action.vault_id.clone(),
                host_id: action.host_id.clone(),
            }),
        );
        cx.activate(false);
        refresh_app_menus(cx);
    });

    let backend_for_select_session = backend.clone();
    cx.on_action(move |action: &SelectSession, cx| {
        let session_id = action.session_id;
        if !with_registered_workspace(cx, |this, _window, cx| {
            if this.backend.session(session_id).is_some() {
                this.select_session(session_id, cx);
            }
        }) {
            let _ = open_workspace_window(
                cx,
                backend_for_select_session.clone(),
                WindowTarget::Session { session_id },
                None,
            );
            cx.activate(false);
        }
        refresh_app_menus(cx);
    });

    let backend_for_switch_theme = backend;
    cx.on_action(move |action: &SwitchTheme, cx| {
        let theme_id = action.theme_id;
        if !with_registered_workspace(cx, |this, window, cx| {
            this.persist_theme(theme_id, window, cx)
        }) {
            let _ = backend_for_switch_theme.set_theme(theme_id.key().to_string());
            let _ = open_workspace_window(
                cx,
                backend_for_switch_theme.clone(),
                WindowTarget::MostRecentOrNew,
                None,
            );
        }
        refresh_app_menus(cx);
    });
}

fn open_workspace_window(
    cx: &mut App,
    backend: UiBackend,
    target: WindowTarget,
    initial_action: Option<InitialWorkspaceAction>,
) -> Result<()> {
    let bootstrap = backend.controller().prepare_window(target)?;
    backend.controller().on_window_opened();
    let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_background: WindowBackgroundAppearance::Blurred,
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Séance".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            ..Default::default()
        },
        move |window, cx| {
            let window_handle = window.window_handle();
            WorkspaceWindowRegistry::register(cx, window_handle);
            let backend = backend.clone();
            let bootstrap = bootstrap.clone();
            let initial_action = initial_action.clone();
            cx.new(move |cx| {
                let entity = cx.entity();
                let focus_handle = cx.focus_handle();
                focus_handle.focus(window);
                let _ = cx.on_release({
                    let window_handle = window_handle;
                    move |_, cx| {
                        WorkspaceWindowRegistry::unregister(cx, window_handle);
                    }
                });

                let mut ws = SeanceWorkspace {
                    focus_handle,
                    active_session_id: bootstrap.attached_session_id,
                    backend: backend.clone(),
                    config: bootstrap.config.clone(),
                    managed_vaults: bootstrap.managed_vaults.clone(),
                    saved_hosts: bootstrap.saved_hosts.clone(),
                    selected_host_id: None,
                    connect_attempts: ConnectAttemptTracker::default(),
                    surface: WorkspaceSurface::Terminal,
                    vault_modal: VaultModalState::new(
                        bootstrap
                            .managed_vaults
                            .iter()
                            .any(|vault| vault.initialized),
                        bootstrap.managed_vaults.iter().any(|vault| vault.unlocked),
                        bootstrap.device_unlock_attempted,
                        bootstrap
                            .managed_vaults
                            .iter()
                            .find_map(|vault| vault.device_unlock_message.as_deref()),
                    ),
                    secure: SecureWorkspaceState::default(),
                    confirm_dialog: None,
                    settings_panel: SettingsPanelState::default(),
                    sftp_browser: None,
                    cached_credentials: bootstrap.cached_credentials.clone(),
                    cached_keys: bootstrap.cached_keys.clone(),
                    update_state: bootstrap.update_state.clone(),
                    active_theme: theme_id_from_config(&bootstrap.config),
                    palette_open: false,
                    palette_query: String::new(),
                    palette_selected: 0,
                    palette_scroll_handle: gpui::ScrollHandle::new(),
                    terminal_metrics: None,
                    last_applied_geometry: None,
                    active_terminal_rows: TerminalGeometry::default().size.rows as usize,
                    terminal_surface: TerminalSurfaceState {
                        theme_id: theme_id_from_config(&bootstrap.config),
                        ..Default::default()
                    },
                    perf_mode_env_override: perf_mode_override_from_env(),
                    perf_overlay: PerfOverlayState::new(perf_mode_from_config(&bootstrap.config)),
                    sidebar_width: crate::model::DEFAULT_SIDEBAR_WIDTH,
                    sidebar_resizing: false,
                    toast: None,
                };
                cx.observe_window_bounds(window, |this: &mut SeanceWorkspace, window, cx| {
                    this.apply_active_terminal_geometry(window);
                    this.invalidate_terminal_surface();
                    this.perf_overlay.mark_input(RedrawReason::TerminalUpdate);
                    cx.notify();
                })
                .detach();
                ws.apply_active_terminal_geometry(window);
                if let Some(notify_rx) = ws
                    .active_session()
                    .and_then(|session| session.take_notify_rx())
                {
                    SeanceWorkspace::schedule_session_watcher(
                        window,
                        cx,
                        entity.clone(),
                        notify_rx,
                    );
                }
                SeanceWorkspace::schedule_config_watcher(
                    window,
                    cx,
                    entity.clone(),
                    backend.subscribe_config_changes(),
                );
                SeanceWorkspace::schedule_update_watcher(
                    window,
                    cx,
                    entity.clone(),
                    backend.subscribe_update_changes(),
                );
                if let Some(initial_action) = initial_action.as_ref() {
                    ws.apply_initial_action(initial_action.clone(), window, cx);
                }
                ws
            })
        },
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promote_unique_moves_item_to_front_without_duplicates() {
        let mut ordered = vec![1_u8, 2, 3];
        promote_unique(&mut ordered, 2);

        assert_eq!(ordered, vec![2, 1, 3]);
    }

    #[test]
    fn promote_unique_inserts_new_item_at_front() {
        let mut ordered = vec![1_u8, 2, 3];
        promote_unique(&mut ordered, 4);

        assert_eq!(ordered, vec![4, 1, 2, 3]);
    }

    #[test]
    fn remove_item_removes_existing_item() {
        let mut ordered = vec![1_u8, 2, 3];
        remove_item(&mut ordered, 2);

        assert_eq!(ordered, vec![1, 3]);
    }

    #[test]
    fn remove_item_ignores_missing_item() {
        let mut ordered = vec![1_u8, 2, 3];
        remove_item(&mut ordered, 9);

        assert_eq!(ordered, vec![1, 2, 3]);
    }
}
