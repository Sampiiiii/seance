use std::{sync::mpsc, thread};

use seance_core::{AppContext, AppControllerHandle, AppPaths};
use seance_platform::{
    InstanceStartup, IpcRequest, PlatformEvent, PlatformRuntime, acquire_or_notify,
    start_ipc_server,
};
use seance_ui::{UiCommand, UiRuntime};

#[cfg(target_os = "macos")]
use seance_platform_macos::{MacosPlatformAppBridge, MacosPlatformRuntime};
#[cfg(not(target_os = "macos"))]
use seance_ui::UiIntegration;

fn main() -> anyhow::Result<()> {
    let paths = AppPaths::detect()?;
    match acquire_or_notify(&paths, IpcRequest::OpenWindow)? {
        InstanceStartup::Secondary(_) => Ok(()),
        InstanceStartup::Primary(_guard) => {
            let context = AppContext::open(paths.clone())?;
            let controller = AppControllerHandle::new(context);
            controller.bootstrap()?;

            let (platform_tx, platform_rx) = mpsc::channel();
            let (ui_tx, ui_rx) = mpsc::channel();
            start_ipc_server(&paths.ipc_socket_path, platform_tx)?;

            let ui_tx_for_ipc = ui_tx.clone();
            thread::spawn(move || {
                while let Ok(event) = platform_rx.recv() {
                    match event {
                        PlatformEvent::OpenWindow => {
                            let _ = ui_tx_for_ipc.send(UiCommand::ActivateApp);
                            let _ = ui_tx_for_ipc.send(UiCommand::OpenWindow {
                                target: seance_core::WindowTarget::MostRecentOrNew,
                            });
                        }
                        PlatformEvent::OpenHost { host_id } => {
                            let _ = ui_tx_for_ipc.send(UiCommand::ActivateApp);
                            let _ = ui_tx_for_ipc.send(UiCommand::OpenHost { host_id });
                        }
                    }
                }
            });

            #[cfg(target_os = "macos")]
            {
                let bridge = MacosPlatformAppBridge::new(controller.clone(), ui_tx.clone());
                MacosPlatformRuntime.run(Box::new(bridge.clone()))?;
                seance_ui::run(UiRuntime {
                    controller,
                    commands: ui_rx,
                    integration: MacosPlatformRuntime::ui_integration(bridge),
                })
            }

            #[cfg(not(target_os = "macos"))]
            {
                ui_tx.send(UiCommand::OpenWindow {
                    target: seance_core::WindowTarget::MostRecentOrNew,
                })?;
                seance_ui::run(UiRuntime {
                    controller,
                    commands: ui_rx,
                    integration: UiIntegration::default(),
                })
            }
        }
    }
}
