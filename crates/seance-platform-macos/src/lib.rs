#![allow(unexpected_cfgs)]

use std::{sync::Arc, sync::mpsc::Sender};

use anyhow::{Context, Result, anyhow};
use gpui::{App, Application, Menu, MenuItem, actions};
use seance_core::{AppControllerHandle, SessionId, SessionKind, WindowTarget};
use seance_platform::{PlatformApp, PlatformRuntime};
use seance_ui::{
    CheckForUpdates, CloseActiveSession, ConnectHost, HideOtherApps, HideSeance, NewTerminal,
    OpenCommandPalette, OpenNewWindow, OpenPreferences, QuitSeance, SelectSession, ShowAllApps,
    SwitchTheme, ThemeId, TogglePerfHud, UiCommand, UiIntegration,
};

#[cfg(target_os = "macos")]
use cocoa::appkit::NSApp;
#[cfg(target_os = "macos")]
use cocoa::base::nil;
#[cfg(target_os = "macos")]
use objc::{msg_send, sel, sel_impl};

actions!(seance_macos, [AboutSeance]);

#[derive(Clone)]
pub struct MacosPlatformAppBridge {
    controller: AppControllerHandle,
    ui_tx: Sender<UiCommand>,
}

impl MacosPlatformAppBridge {
    pub fn new(controller: AppControllerHandle, ui_tx: Sender<UiCommand>) -> Self {
        Self { controller, ui_tx }
    }

    pub fn ui_integration(self) -> UiIntegration {
        let bridge_for_reopen = self.clone();
        let controller_for_install = self.controller.clone();
        let controller_for_refresh = self.controller.clone();
        let refresh_app_menus: Arc<dyn Fn(&mut App) + Send + Sync> =
            Arc::new(move |cx: &mut App| install_macos_menus(cx, &controller_for_refresh));

        UiIntegration {
            configure_application: Some(Box::new(move |application: &Application| {
                let bridge = bridge_for_reopen.clone();
                application.on_reopen(move |_cx| {
                    let _ = bridge.request_reopen();
                });
            })),
            configure_app: Some(Box::new(move |cx: &mut App| {
                cx.on_action(move |_: &AboutSeance, _cx| show_about_panel());
                cx.on_action(move |_: &HideOtherApps, _cx| hide_other_applications());
                cx.on_action(move |_: &ShowAllApps, _cx| show_all_applications());

                cx.set_dock_menu(vec![
                    MenuItem::action("New Window", OpenNewWindow),
                    MenuItem::separator(),
                    MenuItem::action("Quit", QuitSeance),
                ]);
                install_macos_menus(cx, &controller_for_install);
            })),
            refresh_app_menus: Some(refresh_app_menus),
        }
    }

    pub fn quit_app(&self) -> Result<()> {
        self.send(UiCommand::QuitApp)
    }

    pub fn request_open_window(&self) -> Result<()> {
        self.send(UiCommand::OpenWindow {
            target: WindowTarget::MostRecentOrNew,
        })
    }

    pub fn request_reopen(&self) -> Result<()> {
        self.request_show_app()?;
        if self.controller.open_window_count() == 0 {
            self.request_open_window()?;
        }
        Ok(())
    }

    pub fn request_show_app(&self) -> Result<()> {
        self.send(UiCommand::ActivateApp)
    }

    pub fn request_hide_app(&self) -> Result<()> {
        self.send(UiCommand::HideApp)
    }

    fn send(&self, command: UiCommand) -> Result<()> {
        self.ui_tx
            .send(command)
            .map_err(|_| anyhow!("failed to send macOS UI command"))
    }
}

impl PlatformApp for MacosPlatformAppBridge {
    fn on_launch(&mut self) -> Result<()> {
        self.request_open_window()
    }

    fn on_reopen_requested(&mut self) -> Result<()> {
        self.request_reopen()
    }

    fn on_last_window_closed(&mut self) -> Result<seance_core::PlatformCloseAction> {
        Ok(self.controller.on_last_window_closed())
    }

    fn open_window(&mut self) -> Result<()> {
        self.request_open_window()
    }

    fn show_app(&mut self) -> Result<()> {
        self.request_show_app()
    }

    fn hide_app(&mut self) -> Result<()> {
        self.request_hide_app()
    }
}

pub struct MacosPlatformRuntime;

impl MacosPlatformRuntime {
    pub fn ui_integration(bridge: MacosPlatformAppBridge) -> UiIntegration {
        bridge.ui_integration()
    }
}

impl PlatformRuntime for MacosPlatformRuntime {
    fn run(self, mut app: Box<dyn PlatformApp>) -> Result<()> {
        app.on_launch()
            .context("failed to launch macOS platform app")
    }
}

fn install_macos_menus(cx: &mut App, controller: &AppControllerHandle) {
    cx.set_menus(build_macos_menus(controller));
}

fn build_macos_menus(controller: &AppControllerHandle) -> Vec<Menu> {
    vec![
        Menu {
            name: "Séance".into(),
            items: vec![
                MenuItem::action("About Séance", AboutSeance),
                MenuItem::separator(),
                MenuItem::action("Check for Updates…", CheckForUpdates),
                MenuItem::separator(),
                MenuItem::action("Preferences…", OpenPreferences),
                MenuItem::separator(),
                MenuItem::action("Hide Séance", HideSeance),
                MenuItem::action("Hide Others", HideOtherApps),
                MenuItem::action("Show All", ShowAllApps),
                MenuItem::separator(),
                MenuItem::action("Quit Séance", QuitSeance),
            ],
        },
        Menu {
            name: "File".into(),
            items: vec![
                MenuItem::action("New Terminal", NewTerminal),
                MenuItem::separator(),
                MenuItem::submenu(build_hosts_submenu(controller)),
                MenuItem::separator(),
                MenuItem::action("Close Session", CloseActiveSession),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![MenuItem::action("Command Palette…", OpenCommandPalette)],
        },
        Menu {
            name: "View".into(),
            items: vec![
                MenuItem::action("Command Palette…", OpenCommandPalette),
                MenuItem::separator(),
                MenuItem::submenu(build_themes_submenu()),
                MenuItem::separator(),
                MenuItem::action("Performance HUD", TogglePerfHud),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("New Window", OpenNewWindow),
                MenuItem::separator(),
                MenuItem::submenu(build_sessions_submenu(controller)),
            ],
        },
        Menu {
            name: "Help".into(),
            items: Vec::new(),
        },
    ]
}

fn build_hosts_submenu(controller: &AppControllerHandle) -> Menu {
    let mut hosts = controller.list_hosts().unwrap_or_default();
    hosts.sort_by(|left, right| left.label.to_lowercase().cmp(&right.label.to_lowercase()));

    Menu {
        name: "Connect to Host".into(),
        items: hosts
            .into_iter()
            .map(|host| MenuItem::action(host.label, ConnectHost { host_id: host.id }))
            .collect(),
    }
}

fn build_sessions_submenu(controller: &AppControllerHandle) -> Menu {
    let sessions = controller.list_sessions();
    let session_labels = session_menu_labels(controller);

    Menu {
        name: "Active Sessions".into(),
        items: sessions
            .into_iter()
            .zip(session_labels)
            .map(|(session, label)| {
                MenuItem::action(
                    label,
                    SelectSession {
                        session_id: session.id(),
                    },
                )
            })
            .collect(),
    }
}

fn build_themes_submenu() -> Menu {
    Menu {
        name: "Themes".into(),
        items: ThemeId::ALL
            .iter()
            .copied()
            .map(|theme_id| MenuItem::action(theme_id.display_name(), SwitchTheme { theme_id }))
            .collect(),
    }
}

#[cfg(test)]
fn host_menu_labels(controller: &AppControllerHandle) -> Vec<String> {
    let mut hosts = controller.list_hosts().unwrap_or_default();
    hosts.sort_by(|left, right| left.label.to_lowercase().cmp(&right.label.to_lowercase()));
    hosts.into_iter().map(|host| host.label).collect()
}

fn session_menu_labels(controller: &AppControllerHandle) -> Vec<String> {
    let sessions = controller.list_sessions();
    let session_ids = sessions
        .iter()
        .map(|session| session.id())
        .collect::<Vec<_>>();
    let session_kinds = session_ids
        .iter()
        .filter_map(|id| controller.session_kind(*id).map(|kind| (*id, kind)))
        .collect::<std::collections::HashMap<_, _>>();

    sessions
        .iter()
        .map(|session| {
            session_menu_label(&session_ids, &session_kinds, session.id(), session.title())
        })
        .collect()
}

#[cfg(test)]
fn theme_menu_labels() -> Vec<&'static str> {
    ThemeId::ALL.iter().map(ThemeId::display_name).collect()
}

fn session_menu_label(
    session_ids: &[SessionId],
    session_kinds: &std::collections::HashMap<SessionId, SessionKind>,
    target_id: SessionId,
    title: &str,
) -> String {
    match session_kinds.get(&target_id) {
        Some(SessionKind::Local) => {
            let mut local_index = 0;
            for session_id in session_ids {
                if matches!(session_kinds.get(session_id), Some(SessionKind::Local)) {
                    local_index += 1;
                    if *session_id == target_id {
                        return format!("local-{local_index}");
                    }
                }
            }
            title.to_string()
        }
        Some(SessionKind::Remote) | None => title.to_string(),
    }
}

#[allow(unexpected_cfgs)]
fn show_about_panel() {
    #[cfg(target_os = "macos")]
    unsafe {
        let app = NSApp();
        let _: () = msg_send![app, orderFrontStandardAboutPanel: nil];
    }
}

#[allow(unexpected_cfgs)]
fn hide_other_applications() {
    #[cfg(target_os = "macos")]
    unsafe {
        let app = NSApp();
        let _: () = msg_send![app, hideOtherApplications: nil];
    }
}

#[allow(unexpected_cfgs)]
fn show_all_applications() {
    #[cfg(target_os = "macos")]
    unsafe {
        let app = NSApp();
        let _: () = msg_send![app, unhideAllApplications: nil];
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::mpsc,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::{
        MacosPlatformAppBridge, build_macos_menus, host_menu_labels, session_menu_labels,
        theme_menu_labels,
    };
    use anyhow::Result;
    use seance_core::{AppContext, AppControllerHandle, AppPaths};
    use seance_platform::PlatformApp;
    use seance_ui::{ThemeId, UiCommand};
    use seance_vault::{SecretString, VaultHostProfile};

    fn test_controller() -> Result<AppControllerHandle> {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let app_root = std::env::temp_dir().join(format!(
            "seance-macos-tests-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&app_root)?;
        let paths = AppPaths {
            app_root: app_root.clone(),
            config_path: app_root.join("config.toml"),
            vault_db_path: app_root.join("vault.sqlite"),
            ipc_socket_path: app_root.join("resident.sock"),
            instance_lock_path: app_root.join("resident.lock"),
        };
        let context = AppContext::open(paths)?;
        Ok(AppControllerHandle::new(context))
    }

    #[test]
    fn on_launch_enqueues_open_window() -> Result<()> {
        let controller = test_controller()?;
        let (tx, rx) = mpsc::channel();
        let mut bridge = MacosPlatformAppBridge::new(controller, tx);

        bridge.on_launch()?;

        assert_eq!(
            rx.recv().unwrap(),
            UiCommand::OpenWindow {
                target: seance_core::WindowTarget::MostRecentOrNew,
            }
        );
        Ok(())
    }

    #[test]
    fn on_reopen_enqueues_activate_then_open() -> Result<()> {
        let controller = test_controller()?;
        let (tx, rx) = mpsc::channel();
        let mut bridge = MacosPlatformAppBridge::new(controller, tx);

        bridge.on_reopen_requested()?;

        assert_eq!(rx.recv().unwrap(), UiCommand::ActivateApp);
        assert_eq!(
            rx.recv().unwrap(),
            UiCommand::OpenWindow {
                target: seance_core::WindowTarget::MostRecentOrNew,
            }
        );
        Ok(())
    }

    #[test]
    fn quit_app_enqueues_quit_command() -> Result<()> {
        let controller = test_controller()?;
        let (tx, rx) = mpsc::channel();
        let bridge = MacosPlatformAppBridge::new(controller, tx);

        bridge.quit_app()?;

        assert_eq!(rx.recv().unwrap(), UiCommand::QuitApp);
        Ok(())
    }

    #[test]
    fn menu_builder_uses_expected_top_level_names() -> Result<()> {
        let controller = test_controller()?;
        let menus = build_macos_menus(&controller);
        let names = menus
            .into_iter()
            .map(|menu| menu.name.to_string())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            vec!["Séance", "File", "Edit", "View", "Window", "Help"]
        );
        Ok(())
    }

    #[test]
    fn host_menu_contains_saved_hosts() -> Result<()> {
        let controller = test_controller()?;
        controller.create_vault(&SecretString::from("passphrase".to_string()), "Test Device")?;
        controller.save_host(VaultHostProfile {
            id: String::new(),
            label: "prod".into(),
            hostname: "prod.example.com".into(),
            port: 22,
            username: "sam".into(),
            notes: None,
            auth_order: Vec::new(),
        })?;

        assert_eq!(host_menu_labels(&controller), vec!["prod"]);
        Ok(())
    }

    #[test]
    fn session_menu_contains_bootstrapped_sessions() -> Result<()> {
        let controller = test_controller()?;
        controller.bootstrap()?;

        assert_eq!(session_menu_labels(&controller), vec!["local-1"]);
        Ok(())
    }

    #[test]
    fn theme_menu_contains_all_theme_names() {
        let names = theme_menu_labels();
        assert_eq!(names.len(), ThemeId::ALL.len());
        assert!(names.contains(&"Obsidian Smoke"));
        assert!(names.contains(&"Solarized Dark"));
    }
}
