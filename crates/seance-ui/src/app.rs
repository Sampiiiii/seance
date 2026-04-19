use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::Receiver,
};

use anyhow::Result;
use gpui::{
    AnyWindowHandle, App, AppContext, Application, BorrowAppContext, Bounds, Context, Global,
    Window, WindowBackgroundAppearance, WindowBounds, WindowOptions, px, size,
};
use seance_config::AppConfig;
use seance_core::{AppControllerHandle, PlatformCloseAction, WindowTarget};
use seance_terminal::TerminalGeometry;
use tracing::trace;

use crate::{
    CheckForUpdates, CloseActiveSession, ConnectHost, ConnectHostInNewWindow, HideOtherApps,
    HideSeance, NewTerminal, OpenCommandPalette, OpenNewWindow, OpenPreferences, QuitSeance,
    SeanceWorkspace, SelectNextSession, SelectPreviousSession, SelectSession, SelectSessionSlot,
    SettingsSection, ShowAllApps, SwitchTheme, TogglePerfHud,
    backend::UiBackend,
    connect::ConnectAttemptTracker,
    forms::{SecureWorkspaceState, SettingsPanelState, VaultModalState, WorkspaceSurface},
    keybindings::{install_app_keybindings, rebuild_app_keybindings},
    perf::{PerfOverlayState, perf_mode_from_config, perf_mode_override_from_env},
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
        install_app_keybindings(cx, &backend.controller().config_snapshot());
        if let Some(configure_app) = integration.configure_app.take() {
            configure_app(cx);
        }
        refresh_app_menus(cx);

        let async_app = cx.to_async();
        schedule_app_keybinding_watcher(cx, backend.clone(), async_app.clone());
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
        let handled = with_registered_workspace(cx, |this, window, cx| {
            if this.active_session_id != 0 {
                this.close_session(this.active_session_id, window, cx);
            }
        });
        if handled {
            refresh_app_menus(cx);
        }
    });

    cx.on_action(move |_: &SelectPreviousSession, cx| {
        let handled = with_registered_workspace(cx, |this, window, cx| {
            this.select_previous_session(window, cx)
        });
        if handled {
            refresh_app_menus(cx);
        }
    });

    cx.on_action(move |_: &SelectNextSession, cx| {
        let handled =
            with_registered_workspace(cx, |this, window, cx| this.select_next_session(window, cx));
        if handled {
            refresh_app_menus(cx);
        }
    });

    cx.on_action(move |action: &SelectSessionSlot, cx| {
        let handled = with_registered_workspace(cx, |this, window, cx| {
            this.select_session_slot(action.slot, window, cx);
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
        if !with_registered_workspace(cx, |this, window, cx| {
            if this.backend.session(session_id).is_some() {
                this.select_session(session_id, window, cx);
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

fn schedule_app_keybinding_watcher(cx: &mut App, backend: UiBackend, async_app: gpui::AsyncApp) {
    let config_rx = Arc::new(Mutex::new(backend.subscribe_config_changes()));
    cx.foreground_executor()
        .spawn(async move {
            loop {
                let rx = Arc::clone(&config_rx);
                let next_config = async_app
                    .background_executor()
                    .spawn(async move { rx.lock().unwrap().recv().ok() })
                    .await;
                let Some(mut next_config) = next_config else {
                    break;
                };
                while let Ok(config) = config_rx.lock().unwrap().try_recv() {
                    next_config = config;
                }
                let _ = async_app.update(move |cx| {
                    apply_app_keybinding_snapshot(cx, next_config);
                });
            }
        })
        .detach();
}

fn apply_app_keybinding_snapshot(cx: &mut App, config: AppConfig) {
    rebuild_app_keybindings(cx, &config);
    refresh_app_menus(cx);
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
                    cached_port_forwards: bootstrap.cached_port_forwards.clone(),
                    active_port_forwards: bootstrap.active_port_forwards.clone(),
                    update_state: bootstrap.update_state.clone(),
                    active_theme: theme_id_from_config(&bootstrap.config),
                    palette_open: false,
                    palette_query: String::new(),
                    palette_selected: 0,
                    palette_scroll_handle: gpui::ScrollHandle::new(),
                    secure_host_list_scroll_handle: gpui::UniformListScrollHandle::new(),
                    secure_credential_list_scroll_handle: gpui::UniformListScrollHandle::new(),
                    secure_key_list_scroll_handle: gpui::UniformListScrollHandle::new(),
                    secure_auth_available_scroll_handle: gpui::UniformListScrollHandle::new(),
                    secure_filtered_host_indices: Vec::new(),
                    secure_filtered_credential_indices: Vec::new(),
                    secure_filtered_key_indices: Vec::new(),
                    secure_host_search_blobs: Vec::new(),
                    secure_credential_search_blobs: Vec::new(),
                    secure_key_search_blobs: Vec::new(),
                    palette_text_input: crate::TextEditState::default(),
                    secure_text_input: crate::TextEditState::default(),
                    secure_text_target: None,
                    vault_modal_text_input: crate::TextEditState::default(),
                    vault_modal_text_field: None,
                    terminal_metrics: None,
                    last_applied_geometry: None,
                    terminal_resize_epoch: 0,
                    active_terminal_rows: TerminalGeometry::default().size.rows as usize,
                    terminal_surface: TerminalSurfaceState {
                        theme_id: theme_id_from_config(&bootstrap.config),
                        ..Default::default()
                    },
                    terminal_ime: crate::model::TerminalImeState::default(),
                    perf_mode_env_override: perf_mode_override_from_env(),
                    perf_overlay: PerfOverlayState::new(perf_mode_from_config(&bootstrap.config)),
                    sidebar_width: crate::model::DEFAULT_SIDEBAR_WIDTH,
                    sidebar_resizing: false,
                    terminal_selection: None,
                    terminal_drag_anchor: None,
                    terminal_hovered_link: None,
                    terminal_scroll: crate::model::ScrollFrameAccumulator::default(),
                    terminal_scrollbar_hovered: false,
                    terminal_scrollbar_drag: None,
                    frame_pacer: crate::FramePacer::default(),
                    toast: None,
                };
                cx.observe_window_bounds(window, |this: &mut SeanceWorkspace, window, cx| {
                    this.schedule_active_terminal_geometry_refresh(window, cx);
                })
                .detach();
                ws.apply_active_terminal_geometry(window);
                ws.rebuild_secure_search_cache();
                install_workspace_watchers(window, cx, &entity, &mut ws, &backend);
                if let Some(initial_action) = initial_action.as_ref() {
                    ws.apply_initial_action(initial_action.clone(), window, cx);
                }
                if ws.perf_overlay.mode.is_enabled() {
                    ws.ensure_display_probe(window, cx);
                }
                ws
            })
        },
    )?;
    Ok(())
}

#[cfg(not(test))]
fn install_workspace_watchers(
    window: &mut Window,
    cx: &mut Context<SeanceWorkspace>,
    entity: &gpui::Entity<SeanceWorkspace>,
    ws: &mut SeanceWorkspace,
    backend: &UiBackend,
) {
    if let Some(notify_rx) = ws
        .active_session()
        .and_then(|session| session.take_notify_rx())
    {
        SeanceWorkspace::schedule_session_watcher(window, cx, entity.clone(), notify_rx);
    }
    SeanceWorkspace::schedule_config_watcher(
        window,
        cx,
        entity.clone(),
        backend.subscribe_config_changes(),
    );
    SeanceWorkspace::schedule_vault_watcher(
        window,
        cx,
        entity.clone(),
        backend.subscribe_vault_changes(),
    );
    SeanceWorkspace::schedule_update_watcher(
        window,
        cx,
        entity.clone(),
        backend.subscribe_update_changes(),
    );
    SeanceWorkspace::schedule_tunnel_state_watcher(
        window,
        cx,
        entity.clone(),
        backend.subscribe_tunnel_state_changes(),
    );
    SeanceWorkspace::schedule_tunnel_animation(window, cx, entity.clone());
}

#[cfg(test)]
fn install_workspace_watchers(
    _window: &mut Window,
    _cx: &mut Context<SeanceWorkspace>,
    _entity: &gpui::Entity<SeanceWorkspace>,
    _ws: &mut SeanceWorkspace,
    _backend: &UiBackend,
) {
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        sync::{Arc, Mutex, mpsc},
        time::{SystemTime, UNIX_EPOCH},
    };

    use anyhow::Result;
    use gpui::{
        App, AppContext as GpuiAppContext, Entity, KeyDownEvent, Keystroke, Modifiers,
        TestAppContext, VisualTestContext, Window, point, px, size,
    };
    use seance_config::PerfHudDefault;
    use seance_core::{AppContext, AppPaths, SessionOrigin};
    use seance_ssh::{PortForwardRuntimeSnapshot, PortForwardStatus, SshPortForwardMode};
    use seance_terminal::{
        SessionPerfSnapshot, SessionSummary, TerminalKeyEvent, TerminalMouseEvent, TerminalPaste,
        TerminalScrollCommand, TerminalSession, TerminalTextEvent, TerminalViewportSnapshot,
    };
    use seance_vault::{
        HostAuthRef, PortForwardMode, SecretString, VaultHostProfile, VaultPasswordCredential,
        VaultPortForwardProfile,
    };

    use super::*;

    #[derive(Debug)]
    struct RecordingSession {
        id: u64,
        title: String,
        resize_calls: Mutex<Vec<TerminalGeometry>>,
        perf_snapshot: Mutex<SessionPerfSnapshot>,
    }

    impl RecordingSession {
        fn new(id: u64, title: impl Into<String>) -> Self {
            Self {
                id,
                title: title.into(),
                resize_calls: Mutex::new(Vec::new()),
                perf_snapshot: Mutex::new(SessionPerfSnapshot::default()),
            }
        }

        fn resize_calls(&self) -> Vec<TerminalGeometry> {
            self.resize_calls
                .lock()
                .expect("resize calls poisoned")
                .clone()
        }

        fn set_dirty(&self, dirty: bool) {
            self.perf_snapshot
                .lock()
                .expect("perf snapshot poisoned")
                .dirty_since_last_ui_frame = dirty;
        }
    }

    impl TerminalSession for RecordingSession {
        fn id(&self) -> u64 {
            self.id
        }

        fn title(&self) -> &str {
            &self.title
        }

        fn summary(&self) -> SessionSummary {
            SessionSummary::default()
        }

        fn viewport_snapshot(&self) -> TerminalViewportSnapshot {
            TerminalViewportSnapshot::default()
        }

        fn send_input(&self, _bytes: Vec<u8>) -> Result<()> {
            Ok(())
        }

        fn send_text(&self, _event: TerminalTextEvent) -> Result<()> {
            Ok(())
        }

        fn send_key(&self, _event: TerminalKeyEvent) -> Result<()> {
            Ok(())
        }

        fn send_mouse(&self, _event: TerminalMouseEvent) -> Result<()> {
            Ok(())
        }

        fn paste(&self, _paste: TerminalPaste) -> Result<()> {
            Ok(())
        }

        fn resize(&self, geometry: TerminalGeometry) -> Result<()> {
            self.resize_calls
                .lock()
                .expect("resize calls poisoned")
                .push(geometry);
            Ok(())
        }

        fn scroll_viewport(&self, _command: TerminalScrollCommand) -> Result<()> {
            Ok(())
        }

        fn scroll_to_bottom(&self) -> Result<()> {
            Ok(())
        }

        fn perf_snapshot(&self) -> SessionPerfSnapshot {
            let mut snapshot = self.perf_snapshot.lock().expect("perf snapshot poisoned");
            let current = snapshot.clone();
            snapshot.dirty_since_last_ui_frame = false;
            current
        }

        fn take_notify_rx(&self) -> Option<mpsc::Receiver<()>> {
            None
        }
    }

    fn test_root_dir() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock drift")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "seance-ui-window-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(root.join("vaults")).expect("create test vault dir");
        root
    }

    fn make_test_controller() -> AppControllerHandle {
        let root = test_root_dir();
        let context = AppContext::open(AppPaths {
            app_root: root.clone(),
            config_path: root.join("config.toml"),
            diagnostics_dir: root.join("logs"),
            session_logs_dir: root.join("session-logs"),
            vault_db_path: root.join("vault.sqlite"),
            vaults_dir: root.join("vaults"),
            ipc_socket_path: root.join("resident.sock"),
            instance_lock_path: root.join("resident.lock"),
        })
        .expect("open test app context");
        AppControllerHandle::new(context)
    }

    fn seed_saved_host(
        controller: &AppControllerHandle,
    ) -> (
        seance_core::ManagedVaultSummary,
        seance_core::VaultScopedCredentialSummary,
        seance_core::VaultScopedHostSummary,
    ) {
        let vault = controller
            .create_named_vault(
                "Personal".into(),
                &SecretString::from("passphrase".to_string()),
                "device",
            )
            .expect("create vault");
        let credential = controller
            .save_password_credential(
                &vault.vault_id,
                VaultPasswordCredential {
                    id: String::new(),
                    label: "prod password".into(),
                    username_hint: Some("root".into()),
                    secret: "hunter2".into(),
                },
            )
            .expect("save credential");
        let host = controller
            .save_host(
                &vault.vault_id,
                VaultHostProfile {
                    id: String::new(),
                    label: "Production".into(),
                    hostname: "prod.example.com".into(),
                    port: 22,
                    username: "root".into(),
                    notes: None,
                    auth_order: vec![HostAuthRef::Password {
                        credential_id: credential.credential.id.clone(),
                    }],
                },
            )
            .expect("save host");
        (vault, credential, host)
    }

    fn seed_saved_tunnel(
        controller: &AppControllerHandle,
        vault_id: &str,
        host_id: &str,
    ) -> seance_core::VaultScopedPortForwardSummary {
        controller
            .save_port_forward(
                vault_id,
                VaultPortForwardProfile {
                    id: String::new(),
                    host_id: host_id.to_string(),
                    label: "db tunnel".into(),
                    mode: PortForwardMode::Local,
                    listen_address: "127.0.0.1".into(),
                    listen_port: 15432,
                    target_address: "127.0.0.1".into(),
                    target_port: 5432,
                    notes: None,
                },
            )
            .expect("save tunnel")
    }

    fn make_test_controller_with_perf_hud(perf_hud_default: PerfHudDefault) -> AppControllerHandle {
        let controller = make_test_controller();
        controller
            .update_config(|config| {
                config.debug.perf_hud_default = perf_hud_default;
            })
            .expect("update test config");
        controller
    }

    fn close_window(visual: &mut VisualTestContext) {
        visual.executor().allow_parking();
        visual.update(|window: &mut Window, _| window.remove_window());
        visual.run_until_parked();
        visual.executor().forbid_parking();
    }

    fn workspace_root(
        cx: &mut TestAppContext,
        window_handle: AnyWindowHandle,
    ) -> Entity<SeanceWorkspace> {
        cx.update_window(window_handle, |_, window: &mut Window, _| {
            window
                .root::<SeanceWorkspace>()
                .flatten()
                .expect("workspace root")
        })
        .expect("read workspace root")
    }

    fn open_recording_workspace(
        cx: &mut TestAppContext,
    ) -> (
        Arc<RecordingSession>,
        Entity<SeanceWorkspace>,
        VisualTestContext,
    ) {
        let controller = make_test_controller();
        let backend = UiBackend::new(controller.clone()).expect("backend");
        let session = Arc::new(RecordingSession::new(41, "recording"));

        cx.update(|cx: &mut App| {
            cx.set_global(WorkspaceWindowRegistry::default());
            controller.register_remote_session(session.clone());
            open_workspace_window(
                cx,
                backend,
                WindowTarget::Session {
                    session_id: session.id(),
                },
                None,
            )
            .expect("open workspace window");
        });

        let window_handle = cx.windows().into_iter().next().expect("workspace window");
        let workspace = workspace_root(cx, window_handle);
        (
            session,
            workspace,
            VisualTestContext::from_window(window_handle, cx),
        )
    }

    fn open_workspace_with_sessions(
        cx: &mut TestAppContext,
        session_ids: &[u64],
        target_id: u64,
    ) -> (
        Vec<Arc<RecordingSession>>,
        Entity<SeanceWorkspace>,
        VisualTestContext,
    ) {
        let controller = make_test_controller();
        let backend = UiBackend::new(controller.clone()).expect("backend");
        let sessions = session_ids
            .iter()
            .map(|id| Arc::new(RecordingSession::new(*id, format!("session-{id}"))))
            .collect::<Vec<_>>();

        cx.update(|cx: &mut App| {
            cx.set_global(WorkspaceWindowRegistry::default());
            for session in &sessions {
                controller.register_remote_session(session.clone());
            }
            open_workspace_window(
                cx,
                backend,
                WindowTarget::Session {
                    session_id: target_id,
                },
                None,
            )
            .expect("open workspace window");
        });

        let window_handle = cx.windows().into_iter().next().expect("workspace window");
        let workspace = workspace_root(cx, window_handle);
        (
            sessions,
            workspace,
            VisualTestContext::from_window(window_handle, cx),
        )
    }

    fn open_workspace_with_controller(
        cx: &mut TestAppContext,
        controller: AppControllerHandle,
    ) -> (Entity<SeanceWorkspace>, VisualTestContext) {
        let backend = UiBackend::new(controller).expect("backend");

        cx.update(|cx: &mut App| {
            cx.set_global(WorkspaceWindowRegistry::default());
            open_workspace_window(cx, backend, WindowTarget::MostRecentOrNew, None)
                .expect("open workspace window");
        });

        let window_handle = cx.windows().into_iter().next().expect("workspace window");
        let workspace = workspace_root(cx, window_handle);
        (workspace, VisualTestContext::from_window(window_handle, cx))
    }

    fn seed_palette_state(workspace: &Entity<SeanceWorkspace>, visual: &mut VisualTestContext) {
        workspace.update_in(
            visual,
            |this: &mut SeanceWorkspace, _window: &mut Window, _| {
                this.palette_open = true;
                this.palette_query = "theme".into();
                this.palette_selected = 3;
                this.palette_text_input = crate::TextEditState::with_text(&this.palette_query);
                this.palette_scroll_handle
                    .set_offset(point(px(0.0), px(96.0)));
            },
        );
    }

    fn key_event(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: Keystroke {
                modifiers,
                key: key.into(),
                key_char: key_char.map(ToOwned::to_owned),
            },
            is_held: false,
        }
    }

    fn expected_geometry(
        workspace: &Entity<SeanceWorkspace>,
        visual: &mut VisualTestContext,
    ) -> TerminalGeometry {
        workspace.update_in(
            visual,
            |this: &mut SeanceWorkspace, window: &mut Window, _| {
                this.expected_active_terminal_geometry(window)
                    .expect("terminal geometry")
            },
        )
    }

    fn distinct_geometry(current: TerminalGeometry) -> TerminalGeometry {
        let cols = if current.size.cols > 1 {
            current.size.cols - 1
        } else {
            current.size.cols.saturating_add(1)
        };
        let width_px = if cols < current.size.cols {
            current
                .pixel_size
                .width_px
                .saturating_sub(current.cell_width_px)
        } else {
            current
                .pixel_size
                .width_px
                .saturating_add(current.cell_width_px)
        };

        TerminalGeometry::new(
            cols,
            current.size.rows,
            width_px,
            current.pixel_size.height_px,
            current.cell_width_px,
            current.cell_height_px,
        )
        .expect("distinct geometry")
    }

    fn render_workspace_window(visual: &mut VisualTestContext) {
        visual.update(|window: &mut Window, cx| {
            window.refresh();
            window.draw(cx).clear();
        });
    }

    fn schedule_resize_refresh(
        workspace: &Entity<SeanceWorkspace>,
        visual: &mut VisualTestContext,
    ) -> u64 {
        workspace.update_in(
            visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.schedule_active_terminal_geometry_refresh(window, cx);
                this.terminal_resize_epoch
            },
        )
    }

    fn run_scheduled_resize_refresh(
        workspace: &Entity<SeanceWorkspace>,
        visual: &mut VisualTestContext,
        epoch: u64,
    ) {
        workspace.update_in(
            visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.apply_scheduled_terminal_geometry_refresh(epoch, window, cx);
            },
        );
    }

    fn reserve_resize_epoch(
        workspace: &Entity<SeanceWorkspace>,
        visual: &mut VisualTestContext,
    ) -> u64 {
        workspace.update_in(
            visual,
            |this: &mut SeanceWorkspace, _window: &mut Window, _| {
                this.terminal_resize_epoch = this.terminal_resize_epoch.wrapping_add(1);
                this.terminal_resize_epoch
            },
        )
    }

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

    #[gpui::test]
    fn discrete_window_resize_reaches_terminal_without_manual_callback(cx: &mut TestAppContext) {
        let (session, workspace, mut visual) = open_recording_workspace(cx);
        let baseline = session.resize_calls().len();

        visual.simulate_resize(size(px(1440.0), px(900.0)));
        render_workspace_window(&mut visual);

        let resize_calls = session.resize_calls();
        assert_eq!(resize_calls.len(), baseline + 1);
        assert_eq!(
            resize_calls.last().copied(),
            Some(expected_geometry(&workspace, &mut visual))
        );
        close_window(&mut visual);
    }

    #[gpui::test]
    fn window_resize_applies_geometry_on_first_resize(cx: &mut TestAppContext) {
        let (session, workspace, mut visual) = open_recording_workspace(cx);
        let baseline = session.resize_calls().len();

        visual.simulate_resize(size(px(1440.0), px(900.0)));
        let epoch = schedule_resize_refresh(&workspace, &mut visual);
        run_scheduled_resize_refresh(&workspace, &mut visual, epoch);

        let resize_calls = session.resize_calls();
        assert!(resize_calls.len() > baseline);
        assert_eq!(
            resize_calls.last().copied(),
            Some(expected_geometry(&workspace, &mut visual))
        );
        close_window(&mut visual);
    }

    #[gpui::test]
    fn rapid_resizes_coalesce_to_latest_geometry(cx: &mut TestAppContext) {
        let (session, workspace, mut visual) = open_recording_workspace(cx);
        let baseline = session.resize_calls().len();

        let first_epoch = reserve_resize_epoch(&workspace, &mut visual);
        visual.simulate_resize(size(px(1600.0), px(980.0)));
        let second_epoch = reserve_resize_epoch(&workspace, &mut visual);
        run_scheduled_resize_refresh(&workspace, &mut visual, first_epoch);
        run_scheduled_resize_refresh(&workspace, &mut visual, second_epoch);

        let resize_calls = session.resize_calls();
        assert_eq!(resize_calls.len(), baseline + 1);
        assert_eq!(
            resize_calls.last().copied(),
            Some(expected_geometry(&workspace, &mut visual))
        );
        close_window(&mut visual);
    }

    #[gpui::test]
    fn unchanged_geometry_does_not_emit_duplicate_resize(cx: &mut TestAppContext) {
        let (session, workspace, mut visual) = open_recording_workspace(cx);
        let baseline = session.resize_calls().len();

        let first_epoch = schedule_resize_refresh(&workspace, &mut visual);
        let second_epoch = schedule_resize_refresh(&workspace, &mut visual);
        run_scheduled_resize_refresh(&workspace, &mut visual, first_epoch);
        run_scheduled_resize_refresh(&workspace, &mut visual, second_epoch);

        assert_eq!(session.resize_calls().len(), baseline);
        close_window(&mut visual);
    }

    #[gpui::test]
    fn stale_geometry_is_recovered_during_terminal_surface_sync(cx: &mut TestAppContext) {
        let (session, workspace, mut visual) = open_recording_workspace(cx);
        let baseline = session.resize_calls().len();
        let expected = expected_geometry(&workspace, &mut visual);
        let stale = distinct_geometry(expected);

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, _window: &mut Window, _| {
                this.last_applied_geometry = Some(stale);
                this.active_terminal_rows = stale.size.rows as usize;
            },
        );

        render_workspace_window(&mut visual);

        let resize_calls = session.resize_calls();
        assert_eq!(resize_calls.len(), baseline + 1);
        assert_eq!(resize_calls.last().copied(), Some(expected));
        close_window(&mut visual);
    }

    #[gpui::test]
    fn deferred_resize_callback_does_not_duplicate_surface_sync_recovery(cx: &mut TestAppContext) {
        let (session, workspace, mut visual) = open_recording_workspace(cx);
        let baseline = session.resize_calls().len();

        visual.simulate_resize(size(px(1440.0), px(900.0)));
        let epoch = workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, _window: &mut Window, _| this.terminal_resize_epoch,
        );

        render_workspace_window(&mut visual);
        run_scheduled_resize_refresh(&workspace, &mut visual, epoch);

        let resize_calls = session.resize_calls();
        assert_eq!(resize_calls.len(), baseline + 1);
        assert_eq!(
            resize_calls.last().copied(),
            Some(expected_geometry(&workspace, &mut visual))
        );
        close_window(&mut visual);
    }

    #[gpui::test]
    fn selecting_session_consumes_pending_terminal_dirty_state(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let backend = UiBackend::new(controller.clone()).expect("backend");
        let first = Arc::new(RecordingSession::new(41, "first"));
        let second = Arc::new(RecordingSession::new(42, "second"));
        second.set_dirty(true);

        cx.update(|cx: &mut App| {
            cx.set_global(WorkspaceWindowRegistry::default());
            controller.register_remote_session(first.clone());
            controller.register_remote_session(second.clone());
            open_workspace_window(
                cx,
                backend,
                WindowTarget::Session {
                    session_id: first.id(),
                },
                None,
            )
            .expect("open workspace window");
        });

        let window_handle = cx.windows().into_iter().next().expect("workspace window");
        let workspace = workspace_root(cx, window_handle);
        let mut visual = VisualTestContext::from_window(window_handle, cx);

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.select_session(second.id(), window, cx);
            },
        );

        assert!(!second.perf_snapshot().dirty_since_last_ui_frame);
        close_window(&mut visual);
    }

    #[gpui::test]
    fn session_navigation_wraps_between_visible_sessions(cx: &mut TestAppContext) {
        let (sessions, workspace, mut visual) = open_workspace_with_sessions(cx, &[11, 12, 13], 11);
        let first_id = sessions[0].id();
        let last_id = sessions[2].id();

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.select_previous_session(window, cx);
                assert_eq!(this.active_session_id, last_id);
                this.select_next_session(window, cx);
                assert_eq!(this.active_session_id, first_id);
            },
        );

        close_window(&mut visual);
    }

    #[gpui::test]
    fn session_slot_selection_uses_visible_order_and_respects_surface_scope(
        cx: &mut TestAppContext,
    ) {
        let (sessions, workspace, mut visual) = open_workspace_with_sessions(cx, &[21, 22, 23], 21);
        let third_id = sessions[2].id();

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.select_session_slot(3, window, cx);
                assert_eq!(this.active_session_id, third_id);

                this.surface = WorkspaceSurface::Settings;
                this.select_session_slot(1, window, cx);
                assert_eq!(this.active_session_id, third_id);
            },
        );

        close_window(&mut visual);
    }

    #[gpui::test]
    fn startup_with_perf_hud_enabled_does_not_panic(cx: &mut TestAppContext) {
        let controller = make_test_controller_with_perf_hud(PerfHudDefault::Expanded);
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller);

        workspace.update_in(&mut visual, |this: &mut SeanceWorkspace, _, _| {
            assert!(this.perf_overlay.mode.is_enabled());
        });

        close_window(&mut visual);
    }

    #[gpui::test]
    fn close_palette_resets_palette_state(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller);
        seed_palette_state(&workspace, &mut visual);

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, _window: &mut Window, cx| {
                this.close_palette(cx);
                assert!(!this.palette_open);
                assert!(this.palette_query.is_empty());
                assert_eq!(this.palette_selected, 0);
                assert_eq!(this.palette_text_input, crate::TextEditState::default());
            },
        );

        close_window(&mut visual);
    }

    #[gpui::test]
    fn unlock_vault_action_closes_palette_and_leaves_modal_visible(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller);
        seed_palette_state(&workspace, &mut visual);

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.execute_palette_action(crate::palette::PaletteAction::UnlockVault, window, cx);
                assert!(!this.palette_open);
                assert!(this.vault_modal.is_visible());
            },
        );

        close_window(&mut visual);
    }

    #[gpui::test]
    fn add_saved_host_action_closes_palette_and_opens_host_draft_when_unlocked(
        cx: &mut TestAppContext,
    ) {
        let controller = make_test_controller();
        controller
            .create_named_vault(
                "Personal".into(),
                &SecretString::from("passphrase".to_string()),
                "device",
            )
            .expect("create vault");
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller);
        seed_palette_state(&workspace, &mut visual);

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.execute_palette_action(
                    crate::palette::PaletteAction::AddSavedHost,
                    window,
                    cx,
                );
                assert!(!this.palette_open);
                assert_eq!(this.surface, WorkspaceSurface::Secure);
                assert_eq!(this.secure.section, crate::forms::SecureSection::Hosts);
                assert!(this.secure.host_draft.is_some());
                assert!(!this.vault_modal.is_visible());
            },
        );

        close_window(&mut visual);
    }

    #[gpui::test]
    fn add_saved_host_action_closes_palette_and_opens_unlock_modal_when_locked(
        cx: &mut TestAppContext,
    ) {
        let controller = make_test_controller();
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller);
        seed_palette_state(&workspace, &mut visual);

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.execute_palette_action(
                    crate::palette::PaletteAction::AddSavedHost,
                    window,
                    cx,
                );
                assert!(!this.palette_open);
                assert!(this.vault_modal.is_visible());
                assert!(this.secure.host_draft.is_none());
            },
        );

        close_window(&mut visual);
    }

    #[gpui::test]
    fn vault_modal_handle_key_down_mutates_create_name_field(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller);

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.open_vault_modal(
                    crate::forms::UnlockMode::Create,
                    crate::forms::VaultModalOrigin::UserAction,
                    "Create a named encrypted vault.".into(),
                    cx,
                );
                this.vault_modal.vault_name.clear();
                this.sync_vault_modal_text_input();

                let key = key_event("a", Some("a"), Modifiers::default());
                this.handle_key_down(&key, window, cx);

                assert_eq!(this.vault_modal.vault_name.as_str(), "a");
            },
        );

        close_window(&mut visual);
    }

    #[gpui::test]
    fn vault_modal_tab_cycles_fields_in_create_mode(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller);

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.open_vault_modal(
                    crate::forms::UnlockMode::Create,
                    crate::forms::VaultModalOrigin::UserAction,
                    "Create a named encrypted vault.".into(),
                    cx,
                );
                assert_eq!(this.vault_modal.selected_field, 0);

                let tab = key_event("tab", None, Modifiers::default());
                this.handle_key_down(&tab, window, cx);
                assert_eq!(this.vault_modal.selected_field, 1);
                this.handle_key_down(&tab, window, cx);
                assert_eq!(this.vault_modal.selected_field, 2);
                this.handle_key_down(&tab, window, cx);
                assert_eq!(this.vault_modal.selected_field, 0);
            },
        );

        close_window(&mut visual);
    }

    #[gpui::test]
    fn confirm_dialog_handle_key_down_processes_enter_and_escape(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller);

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.surface = WorkspaceSurface::Secure;
                this.secure.section = crate::forms::SecureSection::Hosts;
                this.confirm_dialog = Some(crate::forms::ConfirmDialogState::discard_changes(
                    crate::forms::PendingAction::SwitchSecureSection(
                        crate::forms::SecureSection::Keys,
                    ),
                ));

                let enter = key_event("enter", None, Modifiers::default());
                this.handle_key_down(&enter, window, cx);
                assert!(this.confirm_dialog.is_none());
                assert_eq!(this.secure.section, crate::forms::SecureSection::Keys);

                this.confirm_dialog = Some(crate::forms::ConfirmDialogState::discard_changes(
                    crate::forms::PendingAction::SwitchSecureSection(
                        crate::forms::SecureSection::Tunnels,
                    ),
                ));
                let escape = key_event("escape", None, Modifiers::default());
                this.handle_key_down(&escape, window, cx);
                assert!(this.confirm_dialog.is_none());
                assert_eq!(this.secure.section, crate::forms::SecureSection::Keys);
            },
        );

        close_window(&mut visual);
    }

    #[gpui::test]
    fn open_vault_panel_action_closes_palette_and_switches_surface(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller);
        seed_palette_state(&workspace, &mut visual);

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.execute_palette_action(
                    crate::palette::PaletteAction::OpenVaultPanel,
                    window,
                    cx,
                );
                assert!(!this.palette_open);
                assert_eq!(this.surface, WorkspaceSurface::Secure);
                assert_eq!(
                    this.secure.section,
                    crate::forms::SecureSection::Credentials
                );
            },
        );

        close_window(&mut visual);
    }

    #[gpui::test]
    fn open_tunnel_manager_action_closes_palette_and_switches_surface(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let (vault, _credential, host) = seed_saved_host(&controller);
        let _tunnel = seed_saved_tunnel(&controller, &vault.vault_id, &host.host.id);
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller);
        seed_palette_state(&workspace, &mut visual);

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.execute_palette_action(
                    crate::palette::PaletteAction::OpenTunnelManager,
                    window,
                    cx,
                );
                assert!(!this.palette_open);
                assert_eq!(this.surface, WorkspaceSurface::Secure);
                assert_eq!(this.secure.section, crate::forms::SecureSection::Tunnels);
                assert!(this.secure.tunnel_draft.is_some());
            },
        );

        close_window(&mut visual);
    }

    #[gpui::test]
    fn open_preferences_action_leaves_palette_closed(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller);
        seed_palette_state(&workspace, &mut visual);

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                this.execute_palette_action(
                    crate::palette::PaletteAction::OpenPreferences,
                    window,
                    cx,
                );
                assert!(!this.palette_open);
                assert_eq!(this.surface, WorkspaceSurface::Settings);
                assert!(this.palette_query.is_empty());
            },
        );

        close_window(&mut visual);
    }

    #[gpui::test]
    fn toggling_perf_hud_on_open_workspace_does_not_panic(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller);

        workspace.update_in(
            &mut visual,
            |this: &mut SeanceWorkspace, window: &mut Window, cx| {
                assert!(!this.perf_overlay.mode.is_enabled());
                this.toggle_perf_mode(window, cx);
                assert!(this.perf_overlay.mode.is_enabled());
            },
        );

        close_window(&mut visual);
    }

    #[gpui::test]
    fn refresh_vault_ui_clears_deleted_host_selection(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let (vault, _credential, host) = seed_saved_host(&controller);
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller.clone());
        let host_scope = crate::workspace::host_scope_key(&vault.vault_id, &host.host.id);

        workspace.update_in(&mut visual, |this: &mut SeanceWorkspace, _, _| {
            this.selected_host_id = Some(host_scope.clone());
            this.secure.selected_host_id = Some(host_scope.clone());
        });

        assert!(
            controller
                .delete_host(&vault.vault_id, &host.host.id)
                .expect("delete host")
        );

        workspace.update_in(&mut visual, |this: &mut SeanceWorkspace, _, cx| {
            this.refresh_vault_ui(cx);
            assert!(this.selected_host_id.is_none());
            assert!(this.secure.selected_host_id.is_none());
            assert!(this.saved_hosts.is_empty());
        });

        close_window(&mut visual);
    }

    #[gpui::test]
    fn refresh_vault_ui_turns_deleted_drafts_into_unsaved_clones(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let (vault, _credential, host) = seed_saved_host(&controller);
        let tunnel = seed_saved_tunnel(&controller, &vault.vault_id, &host.host.id);
        let standalone_credential = controller
            .save_password_credential(
                &vault.vault_id,
                VaultPasswordCredential {
                    id: String::new(),
                    label: "standalone".into(),
                    username_hint: Some("deploy".into()),
                    secret: "secret".into(),
                },
            )
            .expect("save standalone credential");
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller.clone());

        let host_scope = crate::workspace::host_scope_key(&vault.vault_id, &host.host.id);
        let tunnel_scope =
            crate::workspace::item_scope_key(&vault.vault_id, &tunnel.port_forward.id);
        let credential_scope =
            crate::workspace::item_scope_key(&vault.vault_id, &standalone_credential.credential.id);

        workspace.update_in(&mut visual, |this: &mut SeanceWorkspace, _, cx| {
            this.activate_tunnel_draft(Some(&tunnel_scope), None, cx);
        });
        assert!(
            controller
                .delete_port_forward(&vault.vault_id, &tunnel.port_forward.id)
                .expect("delete tunnel")
        );
        workspace.update_in(&mut visual, |this: &mut SeanceWorkspace, _, cx| {
            this.refresh_vault_ui(cx);
            let draft = this.secure.tunnel_draft.as_ref().expect("tunnel draft");
            assert!(draft.port_forward_id.is_none());
            assert!(draft.dirty);
        });

        workspace.update_in(&mut visual, |this: &mut SeanceWorkspace, _, cx| {
            this.activate_credential_draft(
                Some(&credential_scope),
                crate::forms::CredentialDraftOrigin::Standalone,
                cx,
            );
        });
        assert!(
            controller
                .delete_password_credential(&vault.vault_id, &standalone_credential.credential.id)
                .expect("delete credential")
        );
        workspace.update_in(&mut visual, |this: &mut SeanceWorkspace, _, cx| {
            this.refresh_vault_ui(cx);
            let draft = this
                .secure
                .credential_draft
                .as_ref()
                .expect("credential draft");
            assert!(draft.credential_id.is_none());
            assert!(draft.dirty);
        });

        workspace.update_in(&mut visual, |this: &mut SeanceWorkspace, _, cx| {
            this.activate_host_draft(Some(&host_scope), cx);
        });
        assert!(
            controller
                .delete_host(&vault.vault_id, &host.host.id)
                .expect("delete host")
        );
        workspace.update_in(&mut visual, |this: &mut SeanceWorkspace, _, cx| {
            this.refresh_vault_ui(cx);
            let draft = this.secure.host_draft.as_ref().expect("host draft");
            assert!(draft.host_id.is_none());
            assert!(draft.dirty);
        });

        close_window(&mut visual);
    }

    #[gpui::test]
    fn live_tunnel_is_marked_deleted_when_saved_rule_disappears(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let (vault, _credential, host) = seed_saved_host(&controller);
        let tunnel = seed_saved_tunnel(&controller, &vault.vault_id, &host.host.id);
        let (workspace, mut visual) = open_workspace_with_controller(cx, controller.clone());
        let tunnel_scope =
            crate::workspace::item_scope_key(&vault.vault_id, &tunnel.port_forward.id);

        workspace.update_in(&mut visual, |this: &mut SeanceWorkspace, _, cx| {
            this.apply_tunnel_state_snapshot(
                vec![PortForwardRuntimeSnapshot {
                    id: tunnel_scope.clone(),
                    vault_id: vault.vault_id.clone(),
                    forward_id: tunnel.port_forward.id.clone(),
                    host_id: host.host.id.clone(),
                    label: tunnel.port_forward.label.clone(),
                    host_label: host.host.label.clone(),
                    mode: SshPortForwardMode::Local,
                    status: PortForwardStatus::Running,
                    listen_address: "127.0.0.1".into(),
                    listen_port: 15432,
                    target_address: "127.0.0.1".into(),
                    target_port: 5432,
                    opened_at: None,
                    active_connections: 0,
                    bytes_in: 0,
                    bytes_out: 0,
                    last_error: None,
                }],
                cx,
            );
            assert!(this.live_tunnel_is_saved(&tunnel_scope));
        });

        assert!(
            controller
                .delete_port_forward(&vault.vault_id, &tunnel.port_forward.id)
                .expect("delete tunnel")
        );

        workspace.update_in(&mut visual, |this: &mut SeanceWorkspace, _, cx| {
            this.refresh_vault_ui(cx);
            assert!(!this.live_tunnel_is_saved(&tunnel_scope));
            assert_eq!(this.active_port_forwards.len(), 1);
        });

        close_window(&mut visual);
    }

    #[gpui::test]
    fn deleted_remote_session_origin_is_detected_after_host_removal(cx: &mut TestAppContext) {
        let controller = make_test_controller();
        let (vault, _credential, host) = seed_saved_host(&controller);
        let backend = UiBackend::new(controller.clone()).expect("backend");
        let session = Arc::new(RecordingSession::new(77, "prod"));

        cx.update(|cx: &mut App| {
            cx.set_global(WorkspaceWindowRegistry::default());
            controller.register_remote_session_with_origin(
                session.clone(),
                SessionOrigin {
                    vault_id: vault.vault_id.clone(),
                    host_id: host.host.id.clone(),
                    host_label_at_connect: host.host.label.clone(),
                },
            );
            open_workspace_window(
                cx,
                backend,
                WindowTarget::Session {
                    session_id: session.id(),
                },
                None,
            )
            .expect("open workspace window");
        });

        let window_handle = cx.windows().into_iter().next().expect("workspace window");
        let workspace = workspace_root(cx, window_handle);
        let mut visual = VisualTestContext::from_window(window_handle, cx);

        assert!(
            controller
                .delete_host(&vault.vault_id, &host.host.id)
                .expect("delete host")
        );

        workspace.update_in(&mut visual, |this: &mut SeanceWorkspace, _, cx| {
            this.refresh_vault_ui(cx);
            let origin = this
                .deleted_remote_session_origin(session.id())
                .expect("deleted origin");
            assert_eq!(origin.host_label_at_connect, "Production");
        });

        close_window(&mut visual);
    }
}
