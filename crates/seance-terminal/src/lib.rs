use std::{
    env,
    io::{Read, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    render::{CellIterator, RowIterator},
    screen::CellWide,
    style::{RgbColor, Style, Underline},
};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 36;
const MAX_RENDERED_LINES: usize = 2_000;

#[derive(Clone, Debug, Default)]
pub struct SessionSnapshot {
    pub lines: Vec<TerminalLine>,
    pub exit_status: Option<String>,
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
    let (command_tx, command_rx) = mpsc::channel();

    let session_snapshot = Arc::clone(&snapshot);
    let error_snapshot = Arc::clone(&snapshot);
    let session_title = Arc::clone(&title);
    thread::Builder::new()
        .name(format!("seance-local-session-{id}"))
        .spawn(move || {
            if let Err(error) = run_local_session(session_snapshot, command_rx) {
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
        command_tx,
    })
}

fn run_local_session(
    snapshot: Arc<Mutex<SessionSnapshot>>,
    command_rx: mpsc::Receiver<SessionCommand>,
) -> Result<()> {
    let pty_system = native_pty_system();
    let pty_pair = pty_system
        .openpty(PtySize {
            rows: DEFAULT_ROWS,
            cols: DEFAULT_COLS,
            pixel_width: 0,
            pixel_height: 0,
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
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
        max_scrollback: 10_000,
    })
    .context("failed to initialize Ghostty terminal")?;
    let mut render_state =
        RenderState::new().context("failed to initialize Ghostty render state")?;
    let mut row_iterator = RowIterator::new().context("failed to create Ghostty row iterator")?;
    let mut cell_iterator =
        CellIterator::new().context("failed to create Ghostty cell iterator")?;

    publish_snapshot(
        &snapshot,
        &mut terminal,
        &mut render_state,
        &mut row_iterator,
        &mut cell_iterator,
        None,
    );

    loop {
        let mut changed = false;

        while let Ok(bytes) = output_rx.try_recv() {
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
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        if changed {
            publish_snapshot(
                &snapshot,
                &mut terminal,
                &mut render_state,
                &mut row_iterator,
                &mut cell_iterator,
                None,
            );
        }

        if let Some(status) = child.try_wait().context("failed to poll shell process")? {
            while let Ok(bytes) = output_rx.try_recv() {
                terminal.vt_write(&bytes);
            }

            publish_snapshot(
                &snapshot,
                &mut terminal,
                &mut render_state,
                &mut row_iterator,
                &mut cell_iterator,
                Some(status.to_string()),
            );
            break;
        }
    }

    Ok(())
}

fn publish_snapshot(
    snapshot: &Arc<Mutex<SessionSnapshot>>,
    terminal: &mut Terminal<'static, 'static>,
    render_state: &mut RenderState<'static>,
    row_iterator: &mut RowIterator<'static>,
    cell_iterator: &mut CellIterator<'static>,
    exit_status: Option<String>,
) {
    let lines = render_styled_lines(terminal, render_state, row_iterator, cell_iterator)
        .unwrap_or_else(|error| {
            vec![TerminalLine {
                spans: vec![TerminalSpan {
                    text: format!("Render error: {error:#}"),
                    style: TerminalCellStyle::default(),
                }],
            }]
        });

    let mut state = snapshot.lock().expect("session snapshot poisoned");
    state.lines = lines;
    if let Some(exit_status) = exit_status {
        state.exit_status = Some(exit_status);
    }
}

fn render_styled_lines(
    terminal: &mut Terminal<'static, 'static>,
    render_state: &mut RenderState<'static>,
    row_iterator: &mut RowIterator<'static>,
    cell_iterator: &mut CellIterator<'static>,
) -> Result<Vec<TerminalLine>> {
    let snapshot = render_state.update(terminal)?;
    let colors = snapshot.colors()?;
    let mut rows = row_iterator.update(&snapshot)?;
    let mut lines = Vec::new();

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
        lines.push(line);
    }

    if lines.len() > MAX_RENDERED_LINES {
        let start = lines.len().saturating_sub(MAX_RENDERED_LINES);
        lines.drain(0..start);
    }

    Ok(lines)
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
}
