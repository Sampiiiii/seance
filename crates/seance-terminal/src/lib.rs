use std::{
    env,
    io::{Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    render::{CellIterator, RowIterator},
    screen::CellWide,
    style::{RgbColor, Style, Underline},
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use tracing::trace;

const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 36;
const DEFAULT_CELL_WIDTH_PX: u16 = 8;
const DEFAULT_CELL_HEIGHT_PX: u16 = 19;
const DEFAULT_PIXEL_WIDTH: u16 = DEFAULT_COLS * DEFAULT_CELL_WIDTH_PX;
const DEFAULT_PIXEL_HEIGHT: u16 = DEFAULT_ROWS * DEFAULT_CELL_HEIGHT_PX;
const MAX_RENDERED_LINES: usize = 2_000;

static SESSION_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Returns a unique session id for any terminal session (local shell, SSH, etc.).
pub fn next_session_id() -> u64 {
    SESSION_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalSize {
    pub cols: u16,
    pub rows: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalPixelSize {
    pub width_px: u16,
    pub height_px: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalGeometry {
    pub size: TerminalSize,
    pub pixel_size: TerminalPixelSize,
    pub cell_width_px: u16,
    pub cell_height_px: u16,
}

impl TerminalGeometry {
    pub fn new(
        cols: u16,
        rows: u16,
        width_px: u16,
        height_px: u16,
        cell_width_px: u16,
        cell_height_px: u16,
    ) -> Result<Self> {
        anyhow::ensure!(cols > 0, "terminal cols must be greater than zero");
        anyhow::ensure!(rows > 0, "terminal rows must be greater than zero");
        anyhow::ensure!(
            cell_width_px > 0,
            "terminal cell width must be greater than zero"
        );
        anyhow::ensure!(
            cell_height_px > 0,
            "terminal cell height must be greater than zero"
        );

        Ok(Self {
            size: TerminalSize { cols, rows },
            pixel_size: TerminalPixelSize {
                width_px,
                height_px,
            },
            cell_width_px,
            cell_height_px,
        })
    }
}

impl Default for TerminalGeometry {
    fn default() -> Self {
        Self {
            size: TerminalSize {
                cols: DEFAULT_COLS,
                rows: DEFAULT_ROWS,
            },
            pixel_size: TerminalPixelSize {
                width_px: DEFAULT_PIXEL_WIDTH,
                height_px: DEFAULT_PIXEL_HEIGHT,
            },
            cell_width_px: DEFAULT_CELL_WIDTH_PX,
            cell_height_px: DEFAULT_CELL_HEIGHT_PX,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SessionSnapshot {
    pub rows: Vec<TerminalRow>,
    pub exit_status: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TerminalRenderMetrics {
    pub snapshot_seq: u64,
    pub last_snapshot_duration: Duration,
    pub avg_snapshot_duration: Duration,
    pub max_snapshot_duration: Duration,
    pub rendered_row_count: usize,
    pub rendered_cell_count: usize,
    pub truncated_row_count: usize,
    pub vt_bytes_processed_since_last_snapshot: usize,
    pub total_vt_bytes_processed: u64,
}

#[derive(Clone, Debug, Default)]
pub struct SessionPerfSnapshot {
    pub terminal: TerminalRenderMetrics,
    pub dirty_since_last_ui_frame: bool,
}

pub trait TerminalSession: Send + Sync {
    fn id(&self) -> u64;
    fn title(&self) -> &str;
    fn snapshot(&self) -> SessionSnapshot;
    fn send_input(&self, bytes: Vec<u8>) -> Result<()>;
    fn resize(&self, geometry: TerminalGeometry) -> Result<()>;
    fn perf_snapshot(&self) -> SessionPerfSnapshot;
    fn take_notify_rx(&self) -> Option<mpsc::Receiver<()>>;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalCellStyle {
    pub foreground: Option<TerminalColor>,
    pub background: Option<TerminalColor>,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub underline: bool,
    pub inverse: bool,
    pub invisible: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalCell {
    pub text: String,
    pub style: TerminalCellStyle,
    pub width: u8,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalRow {
    pub cells: Vec<TerminalCell>,
}

impl TerminalRow {
    pub fn plain_text(&self) -> String {
        self.cells
            .iter()
            .map(|cell| cell.text.as_str())
            .collect::<String>()
    }

    pub fn terminal_width(&self) -> usize {
        self.cells.iter().map(|cell| usize::from(cell.width)).sum()
    }
}

impl From<RgbColor> for TerminalColor {
    fn from(value: RgbColor) -> Self {
        Self {
            r: value.r,
            g: value.g,
            b: value.b,
        }
    }
}

#[derive(Clone)]
pub struct SharedSessionState {
    snapshot: Arc<Mutex<SessionSnapshot>>,
    perf_snapshot: Arc<Mutex<SessionPerfState>>,
    notify_tx: mpsc::SyncSender<()>,
}

impl SharedSessionState {
    pub fn new(initial_message: impl Into<String>) -> (Self, mpsc::Receiver<()>) {
        let (notify_tx, notify_rx) = mpsc::sync_channel(1);
        let state = Self {
            snapshot: Arc::new(Mutex::new(SessionSnapshot {
                rows: vec![TerminalRow {
                    cells: vec![TerminalCell {
                        text: initial_message.into(),
                        style: TerminalCellStyle::default(),
                        width: 1,
                    }],
                }],
                exit_status: None,
            })),
            perf_snapshot: Arc::new(Mutex::new(SessionPerfState::default())),
            notify_tx,
        };
        (state, notify_rx)
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        self.snapshot
            .lock()
            .expect("session snapshot poisoned")
            .clone()
    }

    pub fn perf_snapshot(&self) -> SessionPerfSnapshot {
        let mut perf = self.perf_snapshot.lock().expect("session perf poisoned");
        let snapshot = perf.snapshot.clone();
        perf.snapshot.dirty_since_last_ui_frame = false;
        snapshot
    }

    pub(crate) fn publish_render(
        &self,
        rendered_snapshot: RenderedSnapshot,
        duration: Duration,
        vt_bytes_processed_since_last_snapshot: usize,
        exit_status: Option<String>,
    ) {
        let mut state = self.snapshot.lock().expect("session snapshot poisoned");
        state.rows = rendered_snapshot.rows.clone();
        if let Some(exit_status) = exit_status {
            state.exit_status = Some(exit_status);
        }

        let mut perf = self.perf_snapshot.lock().expect("session perf poisoned");
        perf.snapshot_samples = perf.snapshot_samples.saturating_add(1);
        perf.total_snapshot_duration_ns = perf
            .total_snapshot_duration_ns
            .saturating_add(duration.as_nanos());
        let snapshot_samples = perf.snapshot_samples;
        let avg_snapshot_duration =
            duration_from_nanos(perf.total_snapshot_duration_ns / u128::from(snapshot_samples));

        {
            let metrics = &mut perf.snapshot.terminal;
            metrics.snapshot_seq = snapshot_samples;
            metrics.last_snapshot_duration = duration;
            metrics.avg_snapshot_duration = avg_snapshot_duration;
            metrics.max_snapshot_duration = metrics.max_snapshot_duration.max(duration);
            metrics.rendered_row_count = rendered_snapshot.rows.len();
            metrics.rendered_cell_count = rendered_snapshot.rendered_cell_count;
            metrics.truncated_row_count = rendered_snapshot.truncated_row_count;
            metrics.vt_bytes_processed_since_last_snapshot = vt_bytes_processed_since_last_snapshot;
            metrics.total_vt_bytes_processed = metrics
                .total_vt_bytes_processed
                .saturating_add(vt_bytes_processed_since_last_snapshot as u64);
        }
        perf.snapshot.dirty_since_last_ui_frame = true;

        let metrics = &perf.snapshot.terminal;

        trace!(
            snapshot_seq = metrics.snapshot_seq,
            snapshot_ms = duration.as_secs_f64() * 1_000.0,
            rendered_row_count = metrics.rendered_row_count,
            rendered_cell_count = metrics.rendered_cell_count,
            truncated_row_count = metrics.truncated_row_count,
            vt_bytes_processed_since_last_snapshot,
            "published terminal snapshot"
        );

        let _ = self.notify_tx.try_send(());
    }

    pub fn set_error(&self, error: &anyhow::Error) {
        let mut state = self.snapshot.lock().expect("session snapshot poisoned");
        state.rows = vec![TerminalRow {
            cells: vec![TerminalCell {
                text: format!("Failed to start session: {error:#}"),
                style: TerminalCellStyle::default(),
                width: 1,
            }],
        }];
        state.exit_status = Some("startup error".to_string());
        let _ = self.notify_tx.try_send(());
    }
}

pub struct TerminalEmulator {
    terminal: Terminal<'static, 'static>,
    render_state: RenderState<'static>,
    row_iterator: RowIterator<'static>,
    cell_iterator: CellIterator<'static>,
    pending_vt_bytes: usize,
}

impl TerminalEmulator {
    pub fn new(geometry: TerminalGeometry) -> Result<Self> {
        Ok(Self {
            terminal: Terminal::new(TerminalOptions {
                cols: geometry.size.cols,
                rows: geometry.size.rows,
                max_scrollback: 10_000,
            })
            .context("failed to initialize Ghostty terminal")?,
            render_state: RenderState::new()
                .context("failed to initialize Ghostty render state")?,
            row_iterator: RowIterator::new().context("failed to create Ghostty row iterator")?,
            cell_iterator: CellIterator::new().context("failed to create Ghostty cell iterator")?,
            pending_vt_bytes: 0,
        })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        self.pending_vt_bytes += bytes.len();
        self.terminal.vt_write(bytes);
    }

    pub fn resize(&mut self, geometry: TerminalGeometry) -> Result<()> {
        self.terminal
            .resize(
                geometry.size.cols,
                geometry.size.rows,
                u32::from(geometry.cell_width_px),
                u32::from(geometry.cell_height_px),
            )
            .context("failed to resize Ghostty terminal")
    }

    pub fn publish(&mut self, state: &SharedSessionState, exit_status: Option<String>) {
        let started_at = Instant::now();
        let rendered_snapshot = render_styled_lines(
            &mut self.terminal,
            &mut self.render_state,
            &mut self.row_iterator,
            &mut self.cell_iterator,
        )
        .unwrap_or_else(|error| RenderedSnapshot {
            rows: vec![TerminalRow {
                cells: vec![TerminalCell {
                    text: format!("Render error: {error:#}"),
                    style: TerminalCellStyle::default(),
                    width: 1,
                }],
            }],
            rendered_cell_count: 1,
            truncated_row_count: 0,
        });
        let duration = started_at.elapsed();
        let vt_bytes_processed_since_last_snapshot =
            std::mem::replace(&mut self.pending_vt_bytes, 0);

        state.publish_render(
            rendered_snapshot,
            duration,
            vt_bytes_processed_since_last_snapshot,
            exit_status,
        );
    }
}

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

enum SessionCommand {
    Input(Vec<u8>),
    Resize(TerminalGeometry),
}

#[derive(Debug, Default)]
struct SessionPerfState {
    snapshot: SessionPerfSnapshot,
    snapshot_samples: u64,
    total_snapshot_duration_ns: u128,
}

#[derive(Debug)]
struct RenderedSnapshot {
    rows: Vec<TerminalRow>,
    rendered_cell_count: usize,
    truncated_row_count: usize,
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

fn render_styled_lines(
    terminal: &mut Terminal<'static, 'static>,
    render_state: &mut RenderState<'static>,
    row_iterator: &mut RowIterator<'static>,
    cell_iterator: &mut CellIterator<'static>,
) -> Result<RenderedSnapshot> {
    let snapshot = render_state.update(terminal)?;
    let colors = snapshot.colors()?;
    let mut rows = row_iterator.update(&snapshot)?;
    let mut rendered_rows = Vec::new();
    let mut rendered_cell_count = 0;

    while let Some(row) = rows.next() {
        let mut cells = cell_iterator.update(row)?;
        let mut rendered_row = TerminalRow::default();

        while let Some(cell) = cells.next() {
            let raw_cell = cell.raw_cell()?;
            let width = match raw_cell.wide()? {
                CellWide::Narrow => 1,
                CellWide::Wide => 2,
                CellWide::SpacerTail | CellWide::SpacerHead => 0,
            };
            if width == 0 {
                continue;
            }

            let ghostty_style = cell.style()?;
            let style = normalize_cell_style(
                ghostty_style,
                cell.fg_color()?.map(Into::into),
                cell.bg_color()?.map(Into::into),
                colors.foreground.into(),
                colors.background.into(),
            );
            if style.invisible {
                continue;
            }
            let graphemes = cell.graphemes()?;
            let text = if graphemes.is_empty() {
                " ".to_string()
            } else {
                graphemes.into_iter().collect()
            };

            rendered_row.cells.push(TerminalCell { text, style, width });
        }

        rendered_cell_count += rendered_row.cells.len();
        rendered_rows.push(rendered_row);
    }

    let truncated_row_count = truncate_rendered_rows(&mut rendered_rows);

    Ok(RenderedSnapshot {
        rows: rendered_rows,
        rendered_cell_count,
        truncated_row_count,
    })
}

fn duration_from_nanos(nanos: u128) -> Duration {
    let nanos = nanos.min(u64::MAX as u128) as u64;
    Duration::from_nanos(nanos)
}

fn truncate_rendered_rows(rows: &mut Vec<TerminalRow>) -> usize {
    let truncated_row_count = rows.len().saturating_sub(MAX_RENDERED_LINES);
    if truncated_row_count > 0 {
        let start = rows.len().saturating_sub(MAX_RENDERED_LINES);
        rows.drain(0..start);
    }
    truncated_row_count
}

fn normalize_cell_style(
    ghostty_style: Style,
    foreground: Option<TerminalColor>,
    background: Option<TerminalColor>,
    default_foreground: TerminalColor,
    default_background: TerminalColor,
) -> TerminalCellStyle {
    let mut foreground = foreground;
    let mut background = background;

    if ghostty_style.inverse {
        let original_foreground = foreground;
        let original_background = background;
        foreground = Some(original_background.unwrap_or(default_background));
        background = Some(original_foreground.unwrap_or(default_foreground));
    }

    TerminalCellStyle {
        foreground,
        background,
        bold: ghostty_style.bold,
        italic: ghostty_style.italic,
        faint: ghostty_style.faint,
        underline: ghostty_style.underline != Underline::None,
        inverse: ghostty_style.inverse,
        invisible: ghostty_style.invisible,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_geometry_defaults_are_consistent() {
        let geometry = TerminalGeometry::default();

        assert_eq!(geometry.size.cols, DEFAULT_COLS);
        assert_eq!(geometry.size.rows, DEFAULT_ROWS);
        assert_eq!(geometry.pixel_size.width_px, DEFAULT_PIXEL_WIDTH);
        assert_eq!(geometry.pixel_size.height_px, DEFAULT_PIXEL_HEIGHT);
        assert_eq!(geometry.cell_width_px, DEFAULT_CELL_WIDTH_PX);
        assert_eq!(geometry.cell_height_px, DEFAULT_CELL_HEIGHT_PX);
    }

    #[test]
    fn resize_command_rejects_invalid_geometry() {
        assert!(TerminalGeometry::new(0, 24, 100, 100, 8, 19).is_err());
        assert!(TerminalGeometry::new(80, 0, 100, 100, 8, 19).is_err());
        assert!(TerminalGeometry::new(80, 24, 100, 100, 0, 19).is_err());
        assert!(TerminalGeometry::new(80, 24, 100, 100, 8, 0).is_err());
    }

    fn render_rows_from_vt(vt: &[u8]) -> Vec<TerminalRow> {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            max_scrollback: 10_000,
        })
        .expect("terminal");
        let mut render_state = RenderState::new().expect("render state");
        let mut row_iterator = RowIterator::new().expect("row iterator");
        let mut cell_iterator = CellIterator::new().expect("cell iterator");

        terminal.vt_write(vt);

        render_styled_lines(
            &mut terminal,
            &mut render_state,
            &mut row_iterator,
            &mut cell_iterator,
        )
        .expect("styled lines")
        .rows
    }

    fn render_snapshot_from_vt(vt: &[u8]) -> RenderedSnapshot {
        let mut terminal = Terminal::new(TerminalOptions {
            cols: DEFAULT_COLS,
            rows: DEFAULT_ROWS,
            max_scrollback: 10_000,
        })
        .expect("terminal");
        let mut render_state = RenderState::new().expect("render state");
        let mut row_iterator = RowIterator::new().expect("row iterator");
        let mut cell_iterator = CellIterator::new().expect("cell iterator");

        terminal.vt_write(vt);

        render_styled_lines(
            &mut terminal,
            &mut render_state,
            &mut row_iterator,
            &mut cell_iterator,
        )
        .expect("styled lines")
    }

    fn last_non_empty_row(rows: &[TerminalRow]) -> &TerminalRow {
        rows.iter()
            .rev()
            .find(|row| !row.plain_text().trim().is_empty())
            .expect("non-empty line")
    }

    #[test]
    fn preserves_foreground_colors_per_cell() {
        let rows = render_rows_from_vt(b"\x1b[31mred\x1b[32mgreen\x1b[0m\r\n");
        let row = last_non_empty_row(&rows);

        assert_eq!(row.plain_text().trim_end(), "redgreen");
        assert!(row.cells[0].style.foreground.is_some());
        assert!(row.cells[3].style.foreground.is_some());
        assert_ne!(row.cells[0].style.foreground, row.cells[3].style.foreground);
    }

    #[test]
    fn renders_background_colors_and_preserves_spaces() {
        let rows = render_rows_from_vt(b"\x1b[42m \x1b[0mX\r\n");
        let row = last_non_empty_row(&rows);

        assert_eq!(&row.plain_text()[..2], " X");
        assert!(row.cells[0].style.background.is_some());
    }

    #[test]
    fn captures_bold_italic_and_underline_flags() {
        let rows = render_rows_from_vt(b"\x1b[1mb\x1b[0m\x1b[3mi\x1b[0m\x1b[4mu\x1b[0m\r\n");
        let row = last_non_empty_row(&rows);

        assert!(row.cells[0].style.bold);
        assert!(row.cells[1].style.italic);
        assert!(row.cells[2].style.underline);
    }

    #[test]
    fn preserves_faint_text_for_shell_ghost_text_rendering() {
        let rows = render_rows_from_vt(b"\x1b[2mghost\x1b[0m\r\n");
        let row = last_non_empty_row(&rows);

        assert_eq!(row.plain_text().trim_end(), "ghost");
        assert!(row.cells[..5].iter().all(|cell| cell.style.faint));
    }

    #[test]
    fn normalizes_inverse_colors_for_ui_rendering() {
        let rows = render_rows_from_vt(b"\x1b[31;47mX\x1b[7mY\x1b[0m\r\n");
        let row = last_non_empty_row(&rows);

        assert_eq!(row.cells[0].text, "X");
        assert_eq!(row.cells[1].text, "Y");
        assert_eq!(row.cells[0].style.foreground, row.cells[1].style.background);
        assert_eq!(row.cells[0].style.background, row.cells[1].style.foreground);
        assert!(row.cells[1].style.inverse);
    }

    #[test]
    fn preserves_utf8_graphemes() {
        let rows = render_rows_from_vt("hi 👋 café\r\n".as_bytes());
        let row = last_non_empty_row(&rows);

        assert_eq!(row.plain_text().trim_end(), "hi 👋 café");
        assert!(
            row.cells
                .iter()
                .any(|cell| cell.text == "👋" && cell.width == 2)
        );
    }

    #[test]
    fn preserves_box_drawing_cells() {
        let rows = render_rows_from_vt("┌─┐\r\n│ │\r\n└─┘\r\n".as_bytes());
        let row = rows
            .iter()
            .find(|row| row.plain_text().starts_with("┌─┐"))
            .expect("box drawing row");

        assert_eq!(row.cells[0].text, "┌");
        assert_eq!(row.cells[1].text, "─");
        assert_eq!(row.cells[2].text, "┐");
        assert!(row.cells.iter().all(|cell| cell.width == 1));
    }

    #[test]
    fn preserves_braille_cells() {
        let rows = render_rows_from_vt("⣀⣄⣤⣶\r\n".as_bytes());
        let row = last_non_empty_row(&rows);

        assert_eq!(&row.plain_text()[..("⣀⣄⣤⣶".len())], "⣀⣄⣤⣶");
        assert!(row.cells.iter().all(|cell| cell.width == 1));
    }

    #[test]
    fn preserves_wide_cell_widths() {
        let rows = render_rows_from_vt("A界B\r\n".as_bytes());
        let row = last_non_empty_row(&rows);

        assert_eq!(row.cells[0].text, "A");
        assert_eq!(row.cells[1].text, "界");
        assert_eq!(row.cells[1].width, 2);
        assert_eq!(row.cells[2].text, "B");
        assert_eq!(row.terminal_width(), DEFAULT_COLS as usize);
    }

    #[test]
    fn preserves_trailing_blank_cells_and_right_edge_border() {
        let rows = render_rows_from_vt("│  │".as_bytes());
        let row = last_non_empty_row(&rows);

        assert_eq!(row.cells[0].text, "│");
        assert_eq!(row.cells[3].text, "│");
        assert_eq!(&row.plain_text()[..("│  │".len())], "│  │");
    }

    #[test]
    fn reports_rendered_cell_count() {
        let snapshot = render_snapshot_from_vt(b"\x1b[31mred\x1b[32mgreen\x1b[0m\r\n");

        assert_eq!(
            snapshot.rendered_cell_count,
            usize::from(DEFAULT_COLS) * usize::from(DEFAULT_ROWS)
        );
    }

    #[test]
    fn reports_truncated_row_count() {
        let mut rows = vec![TerminalRow::default(); MAX_RENDERED_LINES + 7];

        let truncated_row_count = truncate_rendered_rows(&mut rows);

        assert_eq!(rows.len(), MAX_RENDERED_LINES);
        assert_eq!(truncated_row_count, 7);
    }

    #[test]
    fn perf_snapshot_acknowledges_dirty_state() {
        let (state, _notify_rx) = SharedSessionState::new("test");
        {
            let mut perf = state.perf_snapshot.lock().unwrap();
            perf.snapshot.dirty_since_last_ui_frame = true;
            perf.snapshot.terminal.snapshot_seq = 3;
        }

        let handle = LocalSessionHandle::new(
            1,
            Arc::<str>::from("test"),
            state,
            mpsc::channel().0,
            _notify_rx,
        );

        let first = handle.perf_snapshot();
        let second = handle.perf_snapshot();

        assert!(first.dirty_since_last_ui_frame);
        assert!(!second.dirty_since_last_ui_frame);
        assert_eq!(second.terminal.snapshot_seq, 3);
    }

    #[test]
    fn duration_average_is_computed_from_nanos() {
        assert_eq!(
            duration_from_nanos(1_500_000),
            Duration::from_nanos(1_500_000)
        );
    }

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
