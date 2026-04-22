use std::{
    sync::{
        Arc, Mutex,
        mpsc::{self as std_mpsc, Receiver},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use seance_terminal::{
    SessionPerfSnapshot, SessionSummary, SharedSessionState, TerminalGeometry,
    TerminalGridSelection, TerminalKeyEvent, TerminalMouseEvent, TerminalPaste,
    TerminalScrollCommand, TerminalSession, TerminalTextEvent, TerminalTurnSnapshot,
    TerminalViewportSnapshot,
};
use tokio::sync::mpsc;

#[derive(Debug)]
pub(crate) enum SessionCommand {
    Input(Vec<u8>),
    Text(TerminalTextEvent),
    Key(TerminalKeyEvent),
    Mouse(TerminalMouseEvent),
    Paste(TerminalPaste),
    Resize(TerminalGeometry),
    ScrollViewport(TerminalScrollCommand),
    ScrollToBottom,
    CopySelectionText {
        selection: TerminalGridSelection,
        reply_tx: std_mpsc::SyncSender<Result<String>>,
    },
    PreviousTurn {
        reply_tx: std_mpsc::SyncSender<Result<Option<TerminalTurnSnapshot>>>,
    },
    CopyActiveScreen {
        reply_tx: std_mpsc::SyncSender<Result<String>>,
    },
}

const ACTIVE_SCREEN_COPY_TIMEOUT: Duration = Duration::from_secs(1);

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

    fn send_text(&self, event: TerminalTextEvent) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Text(event))
            .map_err(|_| anyhow!("failed to forward text event to SSH session"))
    }

    fn send_key(&self, event: TerminalKeyEvent) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Key(event))
            .map_err(|_| anyhow!("failed to forward key event to SSH session"))
    }

    fn send_mouse(&self, event: TerminalMouseEvent) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Mouse(event))
            .map_err(|_| anyhow!("failed to forward mouse event to SSH session"))
    }

    fn paste(&self, paste: TerminalPaste) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Paste(paste))
            .map_err(|_| anyhow!("failed to forward paste to SSH session"))
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

    fn copy_selection_text(&self, selection: TerminalGridSelection) -> Result<String> {
        let (reply_tx, reply_rx) = std_mpsc::sync_channel(1);
        self.command_tx
            .send(SessionCommand::CopySelectionText {
                selection,
                reply_tx,
            })
            .map_err(|_| anyhow!("failed to request selection copy from SSH session"))?;
        match reply_rx.recv_timeout(ACTIVE_SCREEN_COPY_TIMEOUT) {
            Ok(result) => result,
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                Err(anyhow!("timed out waiting for SSH selection copy"))
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => Err(anyhow!(
                "SSH session worker stopped before selection copy completed"
            )),
        }
        .context("failed to copy selected terminal text")
    }

    fn previous_turn(&self) -> Result<Option<TerminalTurnSnapshot>> {
        let (reply_tx, reply_rx) = std_mpsc::sync_channel(1);
        self.command_tx
            .send(SessionCommand::PreviousTurn { reply_tx })
            .map_err(|_| anyhow!("failed to request previous turn from SSH session"))?;
        match reply_rx.recv_timeout(ACTIVE_SCREEN_COPY_TIMEOUT) {
            Ok(result) => result,
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                Err(anyhow!("timed out waiting for SSH previous-turn lookup"))
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => Err(anyhow!(
                "SSH session worker stopped before previous-turn lookup completed"
            )),
        }
        .context("failed to resolve previous terminal turn")
    }

    fn copy_active_screen_text(&self) -> Result<String> {
        let (reply_tx, reply_rx) = std_mpsc::sync_channel(1);
        self.command_tx
            .send(SessionCommand::CopyActiveScreen { reply_tx })
            .map_err(|_| anyhow!("failed to request active-screen copy from SSH session"))?;
        match reply_rx.recv_timeout(ACTIVE_SCREEN_COPY_TIMEOUT) {
            Ok(result) => result,
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                Err(anyhow!("timed out waiting for SSH copy export"))
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => Err(anyhow!(
                "SSH session worker stopped before copy export completed"
            )),
        }
        .context("failed to copy active terminal output")
    }

    fn perf_snapshot(&self) -> SessionPerfSnapshot {
        self.state.perf_snapshot()
    }

    fn take_notify_rx(&self) -> Option<Receiver<()>> {
        self.notify_rx.lock().expect("notify_rx poisoned").take()
    }
}

#[cfg(test)]
mod tests {
    use std::thread;

    use seance_terminal::{SharedSessionState, TerminalGeometry, TerminalGridPoint};

    use super::*;

    #[test]
    fn copy_selection_text_routes_request_through_ssh_worker_channel() {
        let geometry = TerminalGeometry::default();
        let (state, _notify) = SharedSessionState::new("ready", geometry);
        let (command_tx, mut command_rx) = mpsc::unbounded_channel();
        let (_notify_tx, notify_rx) = std_mpsc::channel();
        let handle = SshSessionHandle::new(41, "ssh-41".into(), state, command_tx, notify_rx);

        let worker = thread::spawn(move || {
            let command = command_rx.blocking_recv().expect("command");
            match command {
                SessionCommand::CopySelectionText {
                    selection,
                    reply_tx,
                } => {
                    assert_eq!(selection.anchor.row, 2);
                    assert_eq!(selection.focus.col, 7);
                    let _ = reply_tx.send(Ok("copied".into()));
                }
                other => panic!("unexpected command: {other:?}"),
            }
        });

        let copied = handle
            .copy_selection_text(seance_terminal::TerminalGridSelection {
                anchor: TerminalGridPoint { row: 2, col: 1 },
                focus: TerminalGridPoint { row: 3, col: 7 },
            })
            .expect("copy selection");
        assert_eq!(copied, "copied");
        worker.join().expect("worker join");
    }

    #[test]
    fn previous_turn_routes_request_through_ssh_worker_channel() {
        let geometry = TerminalGeometry::default();
        let (state, _notify) = SharedSessionState::new("ready", geometry);
        let (command_tx, mut command_rx) = mpsc::unbounded_channel();
        let (_notify_tx, notify_rx) = std_mpsc::channel();
        let handle = SshSessionHandle::new(43, "ssh-43".into(), state, command_tx, notify_rx);

        let worker = thread::spawn(move || {
            let command = command_rx.blocking_recv().expect("command");
            match command {
                SessionCommand::PreviousTurn { reply_tx } => {
                    let _ = reply_tx.send(Ok(Some(TerminalTurnSnapshot {
                        turn_id: 14,
                        command_text: "ls".into(),
                        output_text: "file".into(),
                        combined_text: "ls\nfile".into(),
                        start_row: 5,
                        end_row: 6,
                    })));
                }
                other => panic!("unexpected command: {other:?}"),
            }
        });

        let previous = handle.previous_turn().expect("previous turn");
        let previous = previous.expect("expected turn");
        assert_eq!(previous.turn_id, 14);
        assert_eq!(previous.command_text, "ls");
        worker.join().expect("worker join");
    }
}
