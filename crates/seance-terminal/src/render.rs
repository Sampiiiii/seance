use std::time::Instant;

use anyhow::{Context, Result};
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    render::{CellIterator, RowIterator},
    screen::CellWide,
    style::{RgbColor, Style, Underline},
};

use crate::{
    model::{TerminalCell, TerminalCellStyle, TerminalColor, TerminalGeometry, TerminalRow},
    state::{RenderedSnapshot, SharedSessionState},
};

const MAX_RENDERED_LINES: usize = 2_000;

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
        .unwrap_or_else(render_error_snapshot);
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

fn render_error_snapshot(error: anyhow::Error) -> RenderedSnapshot {
    RenderedSnapshot {
        rows: vec![TerminalRow {
            cells: vec![TerminalCell {
                text: format!("Render error: {error:#}"),
                style: TerminalCellStyle::default(),
                width: 1,
            }],
        }],
        rendered_cell_count: 1,
        truncated_row_count: 0,
    }
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
                cell.fg_color()?.map(ghostty_color_to_terminal),
                cell.bg_color()?.map(ghostty_color_to_terminal),
                ghostty_color_to_terminal(colors.foreground),
                ghostty_color_to_terminal(colors.background),
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

fn ghostty_color_to_terminal(value: RgbColor) -> TerminalColor {
    TerminalColor {
        r: value.r,
        g: value.g,
        b: value.b,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_rows_from_vt(vt: &[u8]) -> Vec<TerminalRow> {
        let geometry = TerminalGeometry::default();
        let mut terminal = Terminal::new(TerminalOptions {
            cols: geometry.size.cols,
            rows: geometry.size.rows,
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
        let geometry = TerminalGeometry::default();
        let mut terminal = Terminal::new(TerminalOptions {
            cols: geometry.size.cols,
            rows: geometry.size.rows,
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
        let geometry = TerminalGeometry::default();

        assert_eq!(row.cells[0].text, "A");
        assert_eq!(row.cells[1].text, "界");
        assert_eq!(row.cells[1].width, 2);
        assert_eq!(row.cells[2].text, "B");
        assert_eq!(row.terminal_width(), geometry.size.cols as usize);
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
        let geometry = TerminalGeometry::default();

        assert_eq!(
            snapshot.rendered_cell_count,
            usize::from(geometry.size.cols) * usize::from(geometry.size.rows)
        );
    }

    #[test]
    fn reports_truncated_row_count() {
        let mut rows = vec![TerminalRow::default(); MAX_RENDERED_LINES + 7];

        let truncated_row_count = truncate_rendered_rows(&mut rows);

        assert_eq!(rows.len(), MAX_RENDERED_LINES);
        assert_eq!(truncated_row_count, 7);
    }
}
