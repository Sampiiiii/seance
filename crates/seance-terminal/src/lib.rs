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
    pub lines: Vec<TerminalLine>,
    pub exit_status: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TerminalRenderMetrics {
    pub snapshot_seq: u64,
    pub last_snapshot_duration: Duration,
    pub avg_snapshot_duration: Duration,
    pub max_snapshot_duration: Duration,
    pub rendered_line_count: usize,
    pub rendered_span_count: usize,
    pub truncated_line_count: usize,
    pub vt_bytes_processed_since_last_snapshot: usize,
    pub total_vt_bytes_processed: u64,
}

#[derive(Clone, Debug, Default)]
pub struct SessionPerfSnapshot {
    pub terminal: TerminalRenderMetrics,
    pub dirty_since_last_ui_frame: bool,
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
pub struct TerminalSpan {
    pub text: String,
    pub style: TerminalCellStyle,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalLine {
    pub spans: Vec<TerminalSpan>,
}

impl TerminalLine {
    pub fn plain_text(&self) -> String {
        self.spans
            .iter()
            .map(|span| span.text.as_str())
            .collect::<String>()
    }

    fn trim_trailing_whitespace(&mut self) {
        while let Some(span) = self.spans.last_mut() {
            let trimmed_len = span.text.trim_end_matches(' ').len();
            if trimmed_len == span.text.len() {
                break;
            }

            span.text.truncate(trimmed_len);
            if span.text.is_empty() {
                self.spans.pop();
            } else {
                break;
            }
        }
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
pub struct LocalSessionHandle {
    id: u64,
    title: Arc<str>,
    snapshot: Arc<Mutex<SessionSnapshot>>,
    perf_snapshot: Arc<Mutex<SessionPerfState>>,
    command_tx: mpsc::Sender<SessionCommand>,
}

impl LocalSessionHandle {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn snapshot(&self) -> SessionSnapshot {
        self.snapshot
            .lock()
            .expect("session snapshot poisoned")
            .clone()
    }

    pub fn send_input(&self, bytes: Vec<u8>) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Input(bytes))
            .context("failed to forward input to local shell")
    }

    pub fn resize(&self, geometry: TerminalGeometry) -> Result<()> {
        trace!(?geometry, session_id = self.id, "queueing terminal resize");
        self.command_tx
            .send(SessionCommand::Resize(geometry))
            .context("failed to forward resize to local shell")
    }

    pub fn perf_snapshot(&self) -> SessionPerfSnapshot {
        let mut perf = self.perf_snapshot.lock().expect("session perf poisoned");
        let snapshot = perf.snapshot.clone();
        perf.snapshot.dirty_since_last_ui_frame = false;
        snapshot
    }
}

pub struct LocalSessionFactory {
    next_id: AtomicU64,
}

impl Default for LocalSessionFactory {
    fn default() -> Self {
        Self {
            next_id: AtomicU64::new(1),
        }
    }
}

impl LocalSessionFactory {
    pub fn spawn(&self) -> Result<LocalSessionHandle> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let title: Arc<str> = format!("local-{id}").into();
        spawn_local_session(id, title)
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
    lines: Vec<TerminalLine>,
    rendered_span_count: usize,
    truncated_line_count: usize,
}

fn spawn_local_session(id: u64, title: Arc<str>) -> Result<LocalSessionHandle> {
    let snapshot = Arc::new(Mutex::new(SessionSnapshot {
        lines: vec![TerminalLine {
            spans: vec![TerminalSpan {
                text: "Launching local shell...".to_string(),
                style: TerminalCellStyle::default(),
            }],
        }],
        exit_status: None,
    }));
    let perf_snapshot = Arc::new(Mutex::new(SessionPerfState::default()));
    let (command_tx, command_rx) = mpsc::channel();

    let session_snapshot = Arc::clone(&snapshot);
    let session_perf_snapshot = Arc::clone(&perf_snapshot);
    let error_snapshot = Arc::clone(&snapshot);
    let session_title = Arc::clone(&title);
    thread::Builder::new()
        .name(format!("seance-local-session-{id}"))
        .spawn(move || {
            if let Err(error) =
                run_local_session(session_snapshot, session_perf_snapshot, command_rx)
            {
                let mut state = error_snapshot.lock().expect("session snapshot poisoned");
                state.lines = vec![TerminalLine {
                    spans: vec![TerminalSpan {
                        text: format!("Failed to start session: {error:#}"),
                        style: TerminalCellStyle::default(),
                    }],
                }];
                state.exit_status = Some("startup error".to_string());
            }
        })
        .context("failed to spawn local terminal worker")?;

    Ok(LocalSessionHandle {
        id,
        title: session_title,
        snapshot,
        perf_snapshot,
        command_tx,
    })
}

fn run_local_session(
    snapshot: Arc<Mutex<SessionSnapshot>>,
    perf_snapshot: Arc<Mutex<SessionPerfState>>,
    command_rx: mpsc::Receiver<SessionCommand>,
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

    let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
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

    let mut terminal = Terminal::new(TerminalOptions {
        cols: current_geometry.size.cols,
        rows: current_geometry.size.rows,
        max_scrollback: 10_000,
    })
    .context("failed to initialize Ghostty terminal")?;
    let mut render_state =
        RenderState::new().context("failed to initialize Ghostty render state")?;
    let mut row_iterator = RowIterator::new().context("failed to create Ghostty row iterator")?;
    let mut cell_iterator =
        CellIterator::new().context("failed to create Ghostty cell iterator")?;
    let mut pending_vt_bytes = 0_usize;

    publish_snapshot(
        &snapshot,
        &perf_snapshot,
        &mut terminal,
        &mut render_state,
        &mut row_iterator,
        &mut cell_iterator,
        pending_vt_bytes,
        None,
    );
    pending_vt_bytes = 0;

    loop {
        let mut changed = false;

        while let Ok(bytes) = output_rx.try_recv() {
            pending_vt_bytes += bytes.len();
            terminal.vt_write(&bytes);
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

                terminal
                    .resize(
                        new_geometry.size.cols,
                        new_geometry.size.rows,
                        u32::from(new_geometry.cell_width_px),
                        u32::from(new_geometry.cell_height_px),
                    )
                    .context("failed to resize Ghostty terminal")?;
                trace!(?new_geometry, "applied Ghostty resize");

                current_geometry = new_geometry;
                publish_snapshot(
                    &snapshot,
                    &perf_snapshot,
                    &mut terminal,
                    &mut render_state,
                    &mut row_iterator,
                    &mut cell_iterator,
                    pending_vt_bytes,
                    None,
                );
                pending_vt_bytes = 0;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if changed {
            publish_snapshot(
                &snapshot,
                &perf_snapshot,
                &mut terminal,
                &mut render_state,
                &mut row_iterator,
                &mut cell_iterator,
                pending_vt_bytes,
                None,
            );
            pending_vt_bytes = 0;
        }

        if let Some(status) = child.try_wait().context("failed to poll shell process")? {
            while let Ok(bytes) = output_rx.try_recv() {
                pending_vt_bytes += bytes.len();
                terminal.vt_write(&bytes);
            }

            publish_snapshot(
                &snapshot,
                &perf_snapshot,
                &mut terminal,
                &mut render_state,
                &mut row_iterator,
                &mut cell_iterator,
                pending_vt_bytes,
                Some(status.to_string()),
            );
            break;
        }
    }

    Ok(())
}

fn publish_snapshot(
    snapshot: &Arc<Mutex<SessionSnapshot>>,
    perf_snapshot: &Arc<Mutex<SessionPerfState>>,
    terminal: &mut Terminal<'static, 'static>,
    render_state: &mut RenderState<'static>,
    row_iterator: &mut RowIterator<'static>,
    cell_iterator: &mut CellIterator<'static>,
    vt_bytes_processed_since_last_snapshot: usize,
    exit_status: Option<String>,
) {
    let started_at = Instant::now();
    let rendered_snapshot =
        render_styled_lines(terminal, render_state, row_iterator, cell_iterator).unwrap_or_else(
            |error| RenderedSnapshot {
                lines: vec![TerminalLine {
                    spans: vec![TerminalSpan {
                        text: format!("Render error: {error:#}"),
                        style: TerminalCellStyle::default(),
                    }],
                }],
                rendered_span_count: 1,
                truncated_line_count: 0,
            },
        );
    let duration = started_at.elapsed();

    let mut state = snapshot.lock().expect("session snapshot poisoned");
    state.lines = rendered_snapshot.lines.clone();
    if let Some(exit_status) = exit_status {
        state.exit_status = Some(exit_status);
    }

    let mut perf = perf_snapshot.lock().expect("session perf poisoned");
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
        metrics.rendered_line_count = rendered_snapshot.lines.len();
        metrics.rendered_span_count = rendered_snapshot.rendered_span_count;
        metrics.truncated_line_count = rendered_snapshot.truncated_line_count;
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
        rendered_line_count = metrics.rendered_line_count,
        rendered_span_count = metrics.rendered_span_count,
        truncated_line_count = metrics.truncated_line_count,
        vt_bytes_processed_since_last_snapshot,
        "published terminal snapshot"
    );
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
    let mut lines = Vec::new();
    let mut rendered_span_count = 0;

    while let Some(row) = rows.next() {
        let mut cells = cell_iterator.update(row)?;
        let mut line = TerminalLine::default();

        while let Some(cell) = cells.next() {
            let raw_cell = cell.raw_cell()?;
            if matches!(
                raw_cell.wide()?,
                CellWide::SpacerTail | CellWide::SpacerHead
            ) {
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

            push_span(&mut line.spans, text, style);
        }

        line.trim_trailing_whitespace();
        rendered_span_count += line.spans.len();
        lines.push(line);
    }

    let truncated_line_count = truncate_rendered_lines(&mut lines);

    Ok(RenderedSnapshot {
        lines,
        rendered_span_count,
        truncated_line_count,
    })
}

fn duration_from_nanos(nanos: u128) -> Duration {
    let nanos = nanos.min(u64::MAX as u128) as u64;
    Duration::from_nanos(nanos)
}

fn truncate_rendered_lines(lines: &mut Vec<TerminalLine>) -> usize {
    let truncated_line_count = lines.len().saturating_sub(MAX_RENDERED_LINES);
    if truncated_line_count > 0 {
        let start = lines.len().saturating_sub(MAX_RENDERED_LINES);
        lines.drain(0..start);
    }
    truncated_line_count
}

fn push_span(spans: &mut Vec<TerminalSpan>, text: String, style: TerminalCellStyle) {
    if text.is_empty() {
        return;
    }

    if let Some(previous) = spans.last_mut()
        && previous.style == style
    {
        previous.text.push_str(&text);
        return;
    }

    spans.push(TerminalSpan { text, style });
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

    fn render_lines_from_vt(vt: &[u8]) -> Vec<TerminalLine> {
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
        .lines
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

    fn last_non_empty_line(lines: &[TerminalLine]) -> &TerminalLine {
        lines
            .iter()
            .rev()
            .find(|line| !line.plain_text().is_empty())
            .expect("non-empty line")
    }

    #[test]
    fn renders_foreground_colors_as_distinct_spans() {
        let lines = render_lines_from_vt(b"\x1b[31mred\x1b[32mgreen\x1b[0m\r\n");
        let line = last_non_empty_line(&lines);

        assert_eq!(line.spans.len(), 2);
        assert_eq!(line.spans[0].text, "red");
        assert!(line.spans[0].style.foreground.is_some());
        assert_eq!(line.spans[1].text, "green");
        assert!(line.spans[1].style.foreground.is_some());
        assert_ne!(
            line.spans[0].style.foreground,
            line.spans[1].style.foreground
        );
    }

    #[test]
    fn renders_background_colors_and_preserves_spaces() {
        let lines = render_lines_from_vt(b"\x1b[42m \x1b[0mX\r\n");
        let line = last_non_empty_line(&lines);

        assert_eq!(line.plain_text(), " X");
        assert_eq!(line.spans.len(), 2);
        assert!(line.spans[0].style.background.is_some());
    }

    #[test]
    fn captures_bold_italic_and_underline_flags() {
        let lines = render_lines_from_vt(b"\x1b[1mb\x1b[0m\x1b[3mi\x1b[0m\x1b[4mu\x1b[0m\r\n");
        let line = last_non_empty_line(&lines);

        assert_eq!(line.spans.len(), 3);
        assert!(line.spans[0].style.bold);
        assert!(line.spans[1].style.italic);
        assert!(line.spans[2].style.underline);
    }

    #[test]
    fn preserves_faint_text_for_shell_ghost_text_rendering() {
        let lines = render_lines_from_vt(b"\x1b[2mghost\x1b[0m\r\n");
        let line = last_non_empty_line(&lines);

        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].text, "ghost");
        assert!(line.spans[0].style.faint);
    }

    #[test]
    fn normalizes_inverse_colors_for_ui_rendering() {
        let lines = render_lines_from_vt(b"\x1b[31;47mX\x1b[7mY\x1b[0m\r\n");
        let line = last_non_empty_line(&lines);

        assert_eq!(line.spans.len(), 2);
        assert_eq!(line.spans[0].text, "X");
        assert_eq!(line.spans[1].text, "Y");
        assert_eq!(
            line.spans[0].style.foreground,
            line.spans[1].style.background
        );
        assert_eq!(
            line.spans[0].style.background,
            line.spans[1].style.foreground
        );
        assert!(line.spans[1].style.inverse);
    }

    #[test]
    fn merges_adjacent_cells_with_identical_style() {
        let lines = render_lines_from_vt(b"\x1b[31mhello world\x1b[0m\r\n");
        let line = last_non_empty_line(&lines);

        assert_eq!(line.spans.len(), 1);
        assert_eq!(line.spans[0].text, "hello world");
    }

    #[test]
    fn preserves_utf8_graphemes() {
        let lines = render_lines_from_vt("hi 👋 café\r\n".as_bytes());
        let line = last_non_empty_line(&lines);

        assert_eq!(line.plain_text(), "hi 👋 café");
        assert_eq!(line.spans[0].text.len(), "hi 👋 café".len());
    }

    #[test]
    fn trims_trailing_blank_cells() {
        let lines = render_lines_from_vt(b"hi   ");
        let line = last_non_empty_line(&lines);

        assert_eq!(line.plain_text(), "hi");
    }

    #[test]
    fn reports_rendered_span_count() {
        let snapshot = render_snapshot_from_vt(b"\x1b[31mred\x1b[32mgreen\x1b[0m\r\n");

        assert_eq!(snapshot.rendered_span_count, 2);
    }

    #[test]
    fn reports_truncated_line_count() {
        let mut lines = vec![TerminalLine::default(); MAX_RENDERED_LINES + 7];

        let truncated_line_count = truncate_rendered_lines(&mut lines);

        assert_eq!(lines.len(), MAX_RENDERED_LINES);
        assert_eq!(truncated_line_count, 7);
    }

    #[test]
    fn perf_snapshot_acknowledges_dirty_state() {
        let mut perf = SessionPerfState::default();
        perf.snapshot.dirty_since_last_ui_frame = true;
        perf.snapshot.terminal.snapshot_seq = 3;

        let handle = LocalSessionHandle {
            id: 1,
            title: Arc::<str>::from("test"),
            snapshot: Arc::new(Mutex::new(SessionSnapshot::default())),
            perf_snapshot: Arc::new(Mutex::new(perf)),
            command_tx: mpsc::channel().0,
        };

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
}
