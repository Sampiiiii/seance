use std::sync::{Arc, Mutex, mpsc::Receiver};

use anyhow::{Result, anyhow};
use seance_terminal::{
    SessionPerfSnapshot, SessionSummary, SharedSessionState, TerminalGeometry,
    TerminalScrollCommand, TerminalSession, TerminalViewportSnapshot,
};
use tokio::sync::mpsc;

#[derive(Debug)]
pub(crate) enum SessionCommand {
    Input(Vec<u8>),
    Resize(TerminalGeometry),
    ScrollViewport(TerminalScrollCommand),
    ScrollToBottom,
}

pub struct SshSessionHandle {
    id: u64,
    title: Arc<str>,
    state: SharedSessionState,
    command_tx: mpsc::UnboundedSender<SessionCommand>,
    notify_rx: Mutex<Option<Receiver<()>>>,
}

impl SshSessionHandle {
    pub(crate) fn new(
        id: u64,
        title: String,
        state: SharedSessionState,
        command_tx: mpsc::UnboundedSender<SessionCommand>,
        notify_rx: Receiver<()>,
    ) -> Self {
        Self {
            id,
            title: Arc::<str>::from(title),
            state,
            command_tx,
            notify_rx: Mutex::new(Some(notify_rx)),
        }
    }
}

impl std::fmt::Debug for SshSessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshSessionHandle")
            .field("id", &self.id)
            .field("title", &self.title)
            .finish()
    }
}

impl TerminalSession for SshSessionHandle {
    fn id(&self) -> u64 {
        self.id
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn summary(&self) -> SessionSummary {
        self.state.summary()
    }

    fn viewport_snapshot(&self) -> TerminalViewportSnapshot {
        self.state.viewport_snapshot()
    }

    fn send_input(&self, bytes: Vec<u8>) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Input(bytes))
            .map_err(|_| anyhow!("failed to forward input to SSH session"))
    }

    fn resize(&self, geometry: TerminalGeometry) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Resize(geometry))
            .map_err(|_| anyhow!("failed to forward resize to SSH session"))
    }

    fn scroll_viewport(&self, command: TerminalScrollCommand) -> Result<()> {
        self.command_tx
            .send(SessionCommand::ScrollViewport(command))
            .map_err(|_| anyhow!("failed to forward viewport scroll to SSH session"))
    }

    fn scroll_to_bottom(&self) -> Result<()> {
        self.command_tx
            .send(SessionCommand::ScrollToBottom)
            .map_err(|_| anyhow!("failed to forward viewport bottom command to SSH session"))
    }

    fn perf_snapshot(&self) -> SessionPerfSnapshot {
        self.state.perf_snapshot()
    }

    fn take_notify_rx(&self) -> Option<Receiver<()>> {
        self.notify_rx.lock().expect("notify_rx poisoned").take()
    }
}
