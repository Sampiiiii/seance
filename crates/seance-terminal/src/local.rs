use std::{
    env,
    io::{Read, Write},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tracing::trace;

use crate::{
    model::TerminalGeometry,
    render::TerminalEmulator,
    state::{
        SessionPerfSnapshot, SessionSnapshot, SharedSessionState, TerminalSession, next_session_id,
    },
};

pub struct LocalSessionHandle {
    id: u64,
    title: Arc<str>,
    state: SharedSessionState,
    command_tx: mpsc::Sender<SessionCommand>,
    notify_rx: Mutex<Option<mpsc::Receiver<()>>>,
}

impl LocalSessionHandle {
    pub(crate) fn new(
        id: u64,
        title: Arc<str>,
        state: SharedSessionState,
        command_tx: mpsc::Sender<SessionCommand>,
        notify_rx: mpsc::Receiver<()>,
    ) -> Self {
        Self {
            id,
            title,
            state,
            command_tx,
            notify_rx: Mutex::new(Some(notify_rx)),
        }
    }
}

impl TerminalSession for LocalSessionHandle {
    fn id(&self) -> u64 {
        self.id
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn snapshot(&self) -> SessionSnapshot {
        self.state.snapshot()
    }

    fn send_input(&self, bytes: Vec<u8>) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Input(bytes))
            .context("failed to forward input to local shell")
    }

    fn resize(&self, geometry: TerminalGeometry) -> Result<()> {
        trace!(?geometry, session_id = self.id, "queueing terminal resize");
        self.command_tx
            .send(SessionCommand::Resize(geometry))
            .context("failed to forward resize to local shell")
    }

    fn perf_snapshot(&self) -> SessionPerfSnapshot {
        self.state.perf_snapshot()
    }

    fn take_notify_rx(&self) -> Option<mpsc::Receiver<()>> {
        self.notify_rx.lock().expect("notify_rx poisoned").take()
    }
}

#[derive(Default)]
pub struct LocalSessionFactory;

impl LocalSessionFactory {
    pub fn spawn(&self, shell_override: Option<&str>) -> Result<LocalSessionHandle> {
        let id = next_session_id();
        let title: Arc<str> = format!("local-{id}").into();
        spawn_local_session(id, title, shell_override.map(ToOwned::to_owned))
    }
}

pub(crate) enum SessionCommand {
    Input(Vec<u8>),
    Resize(TerminalGeometry),
}

fn spawn_local_session(
    id: u64,
    title: Arc<str>,
    shell_override: Option<String>,
) -> Result<LocalSessionHandle> {
    let (state, notify_rx) = SharedSessionState::new("Launching local shell...");
    let (command_tx, command_rx) = mpsc::channel();

    let thread_state = state.clone();
    let session_title = Arc::clone(&title);
    thread::Builder::new()
        .name(format!("seance-local-session-{id}"))
        .spawn(move || {
            if let Err(error) = run_local_session(thread_state.clone(), command_rx, shell_override)
            {
                thread_state.set_error(&error);
            }
        })
        .context("failed to spawn local terminal worker")?;

    Ok(LocalSessionHandle::new(
        id,
        session_title,
        state,
        command_tx,
        notify_rx,
    ))
}

fn run_local_session(
    state: SharedSessionState,
    command_rx: mpsc::Receiver<SessionCommand>,
    shell_override: Option<String>,
) -> Result<()> {
    let mut current_geometry = TerminalGeometry::default();
    let pty_system = native_pty_system();
    let pty_pair = pty_system
        .openpty(PtySize {
            rows: current_geometry.size.rows,
            cols: current_geometry.size.cols,
            pixel_width: current_geometry.pixel_size.width_px,
            pixel_height: current_geometry.pixel_size.height_px,
        })
        .context("failed to open PTY")?;

    let shell = shell_override
        .or_else(|| env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/bash".to_string());
    let mut command = CommandBuilder::new(shell);
    command.env("TERM", "xterm-256color");

    let mut child = pty_pair
        .slave
        .spawn_command(command)
        .context("failed to spawn local shell")?;

    let mut writer = pty_pair
        .master
        .take_writer()
        .context("failed to open PTY writer")?;
    let mut reader = pty_pair
        .master
        .try_clone_reader()
        .context("failed to clone PTY reader")?;

    let (output_tx, output_rx) = mpsc::channel();
    thread::Builder::new()
        .name("seance-local-pty-reader".to_string())
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if output_tx.send(buffer[..read].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        })
        .context("failed to spawn PTY reader")?;

    let mut terminal = TerminalEmulator::new(current_geometry)?;
    terminal.publish(&state, None);

    loop {
        let mut changed = false;

        while let Ok(bytes) = output_rx.try_recv() {
            terminal.write(&bytes);
            changed = true;
        }

        match command_rx.recv_timeout(Duration::from_millis(16)) {
            Ok(SessionCommand::Input(bytes)) => {
                writer
                    .write_all(&bytes)
                    .context("failed to write input to PTY")?;
                writer.flush().ok();
            }
            Ok(SessionCommand::Resize(new_geometry)) => {
                if new_geometry == current_geometry {
                    trace!(?new_geometry, "skipping redundant terminal resize");
                    continue;
                }

                pty_pair
                    .master
                    .resize(PtySize {
                        rows: new_geometry.size.rows,
                        cols: new_geometry.size.cols,
                        pixel_width: new_geometry.pixel_size.width_px,
                        pixel_height: new_geometry.pixel_size.height_px,
                    })
                    .context("failed to resize PTY")?;
                trace!(?new_geometry, "applied PTY resize");

                terminal.resize(new_geometry)?;
                trace!(?new_geometry, "applied Ghostty resize");

                current_geometry = new_geometry;
                terminal.publish(&state, None);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if changed {
            terminal.publish(&state, None);
        }

        if let Some(status) = child.try_wait().context("failed to poll shell process")? {
            while let Ok(bytes) = output_rx.try_recv() {
                terminal.write(&bytes);
            }

            terminal.publish(&state, Some(status.to_string()));
            break;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, mpsc};

    use super::*;

    #[test]
    fn local_sessions_fit_terminal_session_trait_objects() {
        let (state, notify_rx) = SharedSessionState::new("hello");
        let handle = Arc::new(LocalSessionHandle::new(
            7,
            Arc::<str>::from("local-7"),
            state,
            mpsc::channel().0,
            notify_rx,
        )) as Arc<dyn TerminalSession>;

        assert_eq!(handle.id(), 7);
        assert_eq!(handle.title(), "local-7");
        assert_eq!(handle.snapshot().rows[0].plain_text(), "hello");
    }
}
