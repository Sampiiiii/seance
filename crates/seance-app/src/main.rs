mod diagnostics;

use std::{sync::mpsc, thread};

use anyhow::Context;
use seance_core::{AppContext, AppControllerHandle, AppPaths};
use seance_platform::{
    InstanceStartup, IpcRequest, PlatformEvent, PlatformRuntime, acquire_or_notify,
    start_ipc_server,
};
use seance_ui::{UiCommand, UiRuntime};
use tracing::{debug, error, info};

#[cfg(target_os = "macos")]
use seance_platform_macos::{MacosPlatformAppBridge, MacosPlatformRuntime};
#[cfg(not(target_os = "macos"))]
use seance_ui::UiIntegration;

fn main() -> anyhow::Result<()> {
    diagnostics::install_panic_hook();

    let result = run();
    if let Err(error) = &result {
        error!(error = %error, "startup failed");
    }
    result
}

fn run() -> anyhow::Result<()> {
    let paths = AppPaths::detect().context("failed during app-path detection")?;
    let diagnostics =
        diagnostics::initialize(&paths).context("failed during diagnostics initialization")?;
    info!(
        pid = std::process::id(),
        log_path = %diagnostics.log_path().display(),
        filter = diagnostics.filter(),
        "process start"
    );
    debug!(app_root = %paths.app_root.display(), "resolved application paths");

    info!("starting single-instance acquisition");
    match acquire_or_notify(&paths, IpcRequest::OpenWindow)
        .context("failed during single-instance acquisition")?
    {
        InstanceStartup::Secondary(_) => Ok(()),
        InstanceStartup::Primary(_guard) => {
            info!("acquired primary instance lock");

            info!("opening application context");
            let context =
                AppContext::open(paths.clone()).context("failed during app-context open")?;
            let controller = AppControllerHandle::new(context);
            info!("bootstrapping application controller");
            controller
                .bootstrap()
                .context("failed during controller bootstrap")?;

            let (platform_tx, platform_rx) = mpsc::channel();
            let (ui_tx, ui_rx) = mpsc::channel();
            info!(socket = %paths.ipc_socket_path.display(), "starting IPC server");
            start_ipc_server(&paths.ipc_socket_path, platform_tx)
                .context("failed during IPC server startup")?;

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
                        PlatformEvent::OpenHost { vault_id, host_id } => {
                            let _ = ui_tx_for_ipc.send(UiCommand::ActivateApp);
                            let _ = ui_tx_for_ipc.send(UiCommand::OpenHost { vault_id, host_id });
                        }
                    }
                }
            });

            #[cfg(target_os = "macos")]
            {
                let bridge = MacosPlatformAppBridge::new(controller.clone(), ui_tx.clone());
                info!("handing off startup to macOS runtime");
                MacosPlatformRuntime
                    .run(Box::new(bridge.clone()))
                    .context("failed during macOS runtime handoff")?;
                info!("starting GPUI application run loop");
                seance_ui::run(UiRuntime {
                    controller,
                    commands: ui_rx,
                    integration: MacosPlatformRuntime::ui_integration(bridge),
                })
                .context("failed during GPUI application run")
            }

            #[cfg(not(target_os = "macos"))]
            {
                info!("issuing first window open request");
                ui_tx
                    .send(UiCommand::OpenWindow {
                        target: seance_core::WindowTarget::MostRecentOrNew,
                    })
                    .context("failed to enqueue first window open request")?;
                info!("starting GPUI application run loop");
                seance_ui::run(UiRuntime {
                    controller,
                    commands: ui_rx,
                    integration: UiIntegration::default(),
                })
                .context("failed during GPUI application run")
            }
        }
    }
}
