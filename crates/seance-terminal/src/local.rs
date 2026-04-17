use std::{
    env,
    io::{Read, Write},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, SystemTime},
};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tracing::{debug, trace};

use crate::{
    SessionPerfSnapshot, SessionSummary, SharedSessionState, TerminalEmulator, TerminalGeometry,
    TerminalKeyEvent, TerminalMouseEvent, TerminalPaste, TerminalScrollCommand, TerminalSession,
    TerminalTextEvent, TerminalTranscriptSink, TerminalViewportSnapshot, TranscriptEvent,
    TranscriptStream, next_session_id,
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

    fn summary(&self) -> SessionSummary {
        self.state.summary()
    }

    fn viewport_snapshot(&self) -> TerminalViewportSnapshot {
        self.state.viewport_snapshot()
    }

    fn send_input(&self, bytes: Vec<u8>) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Input(bytes))
            .context("failed to forward input to local shell")
    }

    fn send_text(&self, event: TerminalTextEvent) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Text(event))
            .context("failed to forward text event to local shell")
    }

    fn send_key(&self, event: TerminalKeyEvent) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Key(event))
            .context("failed to forward key event to local shell")
    }

    fn send_mouse(&self, event: TerminalMouseEvent) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Mouse(event))
            .context("failed to forward mouse event to local shell")
    }

    fn paste(&self, paste: TerminalPaste) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Paste(paste))
            .context("failed to forward paste to local shell")
    }

    fn resize(&self, geometry: TerminalGeometry) -> Result<()> {
        trace!(?geometry, session_id = self.id, "queueing terminal resize");
        self.command_tx
            .send(SessionCommand::Resize(geometry))
            .context("failed to forward resize to local shell")
    }

    fn scroll_viewport(&self, command: TerminalScrollCommand) -> Result<()> {
        self.command_tx
            .send(SessionCommand::ScrollViewport(command))
            .context("failed to forward viewport scroll to local shell")
    }

    fn scroll_to_bottom(&self) -> Result<()> {
        self.command_tx
            .send(SessionCommand::ScrollToBottom)
            .context("failed to forward viewport bottom command to local shell")
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
    pub fn spawn(
        &self,
        shell_override: Option<&str>,
        transcript_sink: Arc<dyn TerminalTranscriptSink>,
    ) -> Result<LocalSessionHandle> {
        let id = next_session_id();
        let title: Arc<str> = format!("local-{id}").into();
        spawn_local_session(
            id,
            title,
            shell_override.map(ToOwned::to_owned),
            transcript_sink,
        )
    }
}

pub(crate) enum SessionCommand {
    Input(Vec<u8>),
    Text(TerminalTextEvent),
    Key(TerminalKeyEvent),
    Mouse(TerminalMouseEvent),
    Paste(TerminalPaste),
    Resize(TerminalGeometry),
    ScrollViewport(TerminalScrollCommand),
    ScrollToBottom,
}

fn spawn_local_session(
    id: u64,
    title: Arc<str>,
    shell_override: Option<String>,
    transcript_sink: Arc<dyn TerminalTranscriptSink>,
) -> Result<LocalSessionHandle> {
    let geometry = TerminalGeometry::default();
    let (state, notify_rx) = SharedSessionState::new("Launching local shell...", geometry);
    let (command_tx, command_rx) = mpsc::channel();

    let thread_state = state.clone();
    let session_title = Arc::clone(&title);
    thread::Builder::new()
        .name(format!("seance-local-session-{id}"))
        .spawn(move || {
            if let Err(error) = run_local_session(
                thread_state.clone(),
                command_rx,
                shell_override,
                transcript_sink,
            ) {
                thread_state.set_error(&error, geometry);
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
    transcript_sink: Arc<dyn TerminalTranscriptSink>,
) -> Result<()> {
    let mut current_geometry = TerminalGeometry::default();
    debug!(?current_geometry, "opening PTY for local shell startup");
    let pty_pair = open_pty(current_geometry).context("failed to open PTY")?;
    debug!("opened PTY for local shell startup");

    let shell = shell_override
        .or_else(|| env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/bash".to_string());
    debug!(shell = %shell, "spawning local shell");
    let mut command = CommandBuilder::new(shell);
    command.env("TERM", "xterm-256color");

    let mut child = pty_pair
        .slave
        .spawn_command(command)
        .context("failed to spawn local shell")?;
    debug!("spawned local shell process");

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

    let mut terminal = TerminalEmulator::new(current_geometry, "Launching local shell...")?;
    terminal.refresh(&state, None, true, transcript_sink.dropped_events());

    loop {
        let mut changed = false;

        while let Ok(bytes) = output_rx.try_recv() {
            transcript_sink.record(TranscriptEvent {
                timestamp: SystemTime::now(),
                stream: TranscriptStream::Output,
                bytes: Arc::from(bytes.as_slice()),
            });
            terminal.write(&bytes);
            changed = true;
        }

        match command_rx.recv_timeout(Duration::from_millis(16)) {
            Ok(SessionCommand::Input(bytes)) => {
                write_input_bytes(&mut writer, &transcript_sink, &bytes)?;
            }
            Ok(SessionCommand::Text(event)) => {
                let bytes = terminal.encode_text_event(&event);
                if !bytes.is_empty() {
                    write_input_bytes(&mut writer, &transcript_sink, &bytes)?;
                }
            }
            Ok(SessionCommand::Key(event)) => {
                let bytes = terminal.encode_key_event(&event)?;
                if !bytes.is_empty() {
                    write_input_bytes(&mut writer, &transcript_sink, &bytes)?;
                }
            }
            Ok(SessionCommand::Mouse(event)) => {
                let bytes = terminal.encode_mouse_event(&event)?;
                if !bytes.is_empty() {
                    write_input_bytes(&mut writer, &transcript_sink, &bytes)?;
                }
            }
            Ok(SessionCommand::Paste(paste)) => {
                let bytes = terminal.encode_paste(&paste);
                if !bytes.is_empty() {
                    write_input_bytes(&mut writer, &transcript_sink, &bytes)?;
                }
            }
            Ok(SessionCommand::Resize(new_geometry)) => {
                if new_geometry == current_geometry {
                    trace!(?new_geometry, "skipping redundant terminal resize");
                    continue;
                }

                resize_pty(&*pty_pair.master, new_geometry).context("failed to resize PTY")?;
                trace!(?new_geometry, "applied PTY resize");

                terminal.resize(new_geometry)?;
                trace!(?new_geometry, "applied Ghostty resize");

                current_geometry = new_geometry;
                terminal.refresh(&state, None, true, transcript_sink.dropped_events());
            }
            Ok(SessionCommand::ScrollViewport(command)) => {
                terminal.scroll_viewport(command);
                terminal.refresh(&state, None, false, transcript_sink.dropped_events());
            }
            Ok(SessionCommand::ScrollToBottom) => {
                terminal.scroll_viewport(TerminalScrollCommand::Bottom);
                terminal.refresh(&state, None, false, transcript_sink.dropped_events());
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if changed {
            terminal.refresh(&state, None, false, transcript_sink.dropped_events());
        }

        if let Some(status) = child.try_wait().context("failed to poll shell process")? {
            while let Ok(bytes) = output_rx.try_recv() {
                transcript_sink.record(TranscriptEvent {
                    timestamp: SystemTime::now(),
                    stream: TranscriptStream::Output,
                    bytes: Arc::from(bytes.as_slice()),
                });
                terminal.write(&bytes);
            }

            terminal.refresh(
                &state,
                Some(status.to_string()),
                true,
                transcript_sink.dropped_events(),
            );
            break;
        }
    }

    Ok(())
}

fn write_input_bytes(
    writer: &mut dyn Write,
    transcript_sink: &Arc<dyn TerminalTranscriptSink>,
    bytes: &[u8],
) -> Result<()> {
    transcript_sink.record(TranscriptEvent {
        timestamp: SystemTime::now(),
        stream: TranscriptStream::Input,
        bytes: Arc::from(bytes),
    });
    writer
        .write_all(bytes)
        .context("failed to write input to PTY")?;
    writer.flush().ok();
    Ok(())
}

fn pty_size_from_geometry(geometry: TerminalGeometry) -> PtySize {
    PtySize {
        rows: geometry.size.rows,
        cols: geometry.size.cols,
        pixel_width: geometry.cell_width_px,
        pixel_height: geometry.cell_height_px,
    }
}

fn pty_size_without_pixels(geometry: TerminalGeometry) -> PtySize {
    PtySize {
        pixel_width: 0,
        pixel_height: 0,
        ..pty_size_from_geometry(geometry)
    }
}

fn should_retry_without_pixels(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .and_then(std::io::Error::raw_os_error)
            == Some(22)
            || cause
                .to_string()
                .to_ascii_lowercase()
                .contains("invalid value")
    })
}

fn open_pty(geometry: TerminalGeometry) -> Result<portable_pty::PtyPair> {
    let pty_system = native_pty_system();
    let size = pty_size_from_geometry(geometry);

    match pty_system.openpty(size) {
        Ok(pair) => Ok(pair),
        Err(error) if should_retry_without_pixels(&error) => {
            trace!(
                ?geometry,
                "retrying PTY open without pixel metrics after invalid value"
            );
            pty_system.openpty(pty_size_without_pixels(geometry))
        }
        Err(error) => Err(error),
    }
}

fn resize_pty(master: &dyn MasterPty, geometry: TerminalGeometry) -> Result<()> {
    let size = pty_size_from_geometry(geometry);

    match master.resize(size) {
        Ok(()) => Ok(()),
        Err(error) if should_retry_without_pixels(&error) => {
            trace!(
                ?geometry,
                "retrying PTY resize without pixel metrics after invalid value"
            );
            master.resize(pty_size_without_pixels(geometry))
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        env,
        path::Path,
        sync::{Arc, Mutex, mpsc},
        thread,
        time::{Duration, Instant},
    };

    use super::*;
    use crate::NoopTranscriptSink;

    #[test]
    fn local_sessions_fit_terminal_session_trait_objects() {
        let geometry = TerminalGeometry::default();
        let (state, notify_rx) = SharedSessionState::new("hello", geometry);
        let handle = Arc::new(LocalSessionHandle::new(
            7,
            Arc::<str>::from("local-7"),
            state,
            mpsc::channel().0,
            notify_rx,
        )) as Arc<dyn TerminalSession>;

        assert_eq!(handle.id(), 7);
        assert_eq!(handle.title(), "local-7");
        assert_eq!(handle.summary().preview_line, "hello");
    }

    #[test]
    fn pty_size_uses_cell_metrics_instead_of_total_viewport_pixels() {
        let geometry = TerminalGeometry::new(132, 48, 1400, 900, 9, 21).expect("geometry");

        let size = pty_size_from_geometry(geometry);

        assert_eq!(size.rows, 48);
        assert_eq!(size.cols, 132);
        assert_eq!(size.pixel_width, 9);
        assert_eq!(size.pixel_height, 21);
    }

    #[test]
    fn pty_size_without_pixels_preserves_rows_and_cols() {
        let geometry = TerminalGeometry::new(90, 30, 1024, 768, 8, 19).expect("geometry");

        let size = pty_size_without_pixels(geometry);

        assert_eq!(size.rows, 30);
        assert_eq!(size.cols, 90);
        assert_eq!(size.pixel_width, 0);
        assert_eq!(size.pixel_height, 0);
    }

    #[test]
    fn invalid_value_errors_trigger_pixel_retry() {
        let error = anyhow::anyhow!(std::io::Error::from_raw_os_error(22));

        assert!(should_retry_without_pixels(&error));
    }

    #[cfg(unix)]
    fn test_shell() -> String {
        if Path::new("/bin/sh").exists() {
            "/bin/sh".to_string()
        } else {
            env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string())
        }
    }

    #[cfg(unix)]
    fn test_echo_program() -> String {
        if Path::new("/bin/cat").exists() {
            "/bin/cat".to_string()
        } else {
            test_shell()
        }
    }

    #[cfg(unix)]
    fn wait_for_condition(
        session: &dyn TerminalSession,
        notify_rx: &Option<mpsc::Receiver<()>>,
        timeout: Duration,
        predicate: impl Fn(&dyn TerminalSession) -> bool,
    ) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate(session) {
                return true;
            }

            if let Some(rx) = notify_rx {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let _ = rx.recv_timeout(remaining.min(Duration::from_millis(50)));
            } else {
                thread::sleep(Duration::from_millis(50));
            }
        }

        predicate(session)
    }

    #[derive(Default)]
    struct RecordingSink {
        output: Mutex<Vec<u8>>,
    }

    impl RecordingSink {
        fn output_contains(&self, needle: &str) -> bool {
            let bytes = self.output.lock().expect("output poisoned");
            String::from_utf8_lossy(&bytes).contains(needle)
        }
    }

    impl TerminalTranscriptSink for RecordingSink {
        fn record(&self, event: TranscriptEvent) {
            if matches!(event.stream, TranscriptStream::Output) {
                self.output
                    .lock()
                    .expect("output poisoned")
                    .extend_from_slice(&event.bytes);
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn spawned_local_session_does_not_enter_startup_error() {
        let session = LocalSessionFactory
            .spawn(Some(&test_shell()), Arc::new(NoopTranscriptSink))
            .expect("spawn local session");
        let notify_rx = session.take_notify_rx();

        assert!(wait_for_condition(
            &session,
            &notify_rx,
            Duration::from_secs(2),
            |session| session.summary().exit_status.as_deref() != Some("startup error"),
        ));
        assert_ne!(
            session.summary().exit_status.as_deref(),
            Some("startup error")
        );

        let _ = session.send_input(b"exit\n".to_vec());
    }

    #[cfg(unix)]
    #[test]
    fn local_session_accepts_immediate_resize_with_ui_geometry() {
        let session = LocalSessionFactory
            .spawn(Some(&test_shell()), Arc::new(NoopTranscriptSink))
            .expect("spawn local session");
        let notify_rx = session.take_notify_rx();
        let ui_geometry = TerminalGeometry::new(123, 41, 988, 788, 8, 19).expect("ui geometry");

        session.resize(ui_geometry).expect("queue resize");

        assert!(wait_for_condition(
            &session,
            &notify_rx,
            Duration::from_secs(2),
            |session| session.summary().exit_status.as_deref() != Some("startup error"),
        ));
        assert_ne!(
            session.summary().exit_status.as_deref(),
            Some("startup error")
        );

        let _ = session.send_input(b"exit\n".to_vec());
    }

    #[cfg(unix)]
    #[test]
    fn local_session_echo_round_trip_works() {
        let sink = Arc::new(RecordingSink::default());
        let session = LocalSessionFactory
            .spawn(Some(&test_echo_program()), sink.clone())
            .expect("spawn local session");
        let notify_rx = session.take_notify_rx();
        let marker = "__SEANCE_LOCAL_ECHO__";

        session
            .send_input(format!("{marker}\n").into_bytes())
            .expect("send input");

        assert!(wait_for_condition(
            &session,
            &notify_rx,
            Duration::from_secs(3),
            |_| sink.output_contains(marker),
        ));
    }
}
