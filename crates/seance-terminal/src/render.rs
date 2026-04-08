use std::{sync::Arc, time::Instant};

use anyhow::{Context, Result};
use libghostty_vt::{
    RenderState, Terminal, TerminalOptions,
    render::{CellIterator, Dirty, RowIterator},
    screen::CellWide,
    style::{RgbColor, Style, Underline},
    terminal::{Mode, ScrollViewport},
};
use tracing::trace;

use crate::{
    SessionSummary, SharedSessionState, TerminalCell, TerminalCellStyle, TerminalColor,
    TerminalCursor, TerminalGeometry, TerminalRow, TerminalScreenKind, TerminalScrollCommand,
    state::PublishedViewport, viewport::ViewportCache,
};

const MAX_SCROLLBACK_LINES: usize = 10_000;

pub struct TerminalEmulator {
    terminal: Terminal<'static, 'static>,
    render_state: RenderState<'static>,
    row_iterator: RowIterator<'static>,
    cell_iterator: CellIterator<'static>,
    pending_vt_bytes: usize,
    viewport_cache: ViewportCache,
}

impl TerminalEmulator {
    pub fn new(geometry: TerminalGeometry, initial_message: impl Into<String>) -> Result<Self> {
        let initial_message = initial_message.into();
        Ok(Self {
            terminal: Terminal::new(TerminalOptions {
                cols: geometry.size.cols,
                rows: geometry.size.rows,
                max_scrollback: MAX_SCROLLBACK_LINES,
            })
            .context("failed to initialize Ghostty terminal")?,
            render_state: RenderState::new()
                .context("failed to initialize Ghostty render state")?,
            row_iterator: RowIterator::new().context("failed to create Ghostty row iterator")?,
            cell_iterator: CellIterator::new().context("failed to create Ghostty cell iterator")?,
            pending_vt_bytes: 0,
            viewport_cache: ViewportCache::new(
                geometry,
                TerminalRow {
                    cells: vec![TerminalCell {
                        text: initial_message,
                        style: TerminalCellStyle::default(),
                        width: 1,
                    }],
                },
            ),
        })
    }

    pub fn write(&mut self, bytes: &[u8]) {
        self.pending_vt_bytes += bytes.len();
        self.terminal.vt_write(bytes);
    }

    pub fn resize(&mut self, geometry: TerminalGeometry) -> Result<()> {
        self.viewport_cache.resize(geometry);
        self.terminal
            .resize(
                geometry.size.cols,
                geometry.size.rows,
                u32::from(geometry.cell_width_px),
                u32::from(geometry.cell_height_px),
            )
            .context("failed to resize Ghostty terminal")
    }

    pub fn scroll_viewport(&mut self, command: TerminalScrollCommand) {
        let delta = match command {
            TerminalScrollCommand::Top => Some(ScrollViewport::Top),
            TerminalScrollCommand::Bottom => Some(ScrollViewport::Bottom),
            TerminalScrollCommand::DeltaRows(delta) => Some(ScrollViewport::Delta(delta)),
            TerminalScrollCommand::PageUp => Some(ScrollViewport::Delta(
                -(self.viewport_cache.geometry().size.rows.saturating_sub(1) as isize),
            )),
            TerminalScrollCommand::PageDown => Some(ScrollViewport::Delta(
                self.viewport_cache.geometry().size.rows.saturating_sub(1) as isize,
            )),
        };

        if let Some(delta) = delta {
            self.terminal.scroll_viewport(delta);
            self.viewport_cache.at_bottom = matches!(
                command,
                TerminalScrollCommand::Bottom | TerminalScrollCommand::PageDown
            );
        }
    }

    pub fn refresh(
        &mut self,
        state: &SharedSessionState,
        exit_status: Option<String>,
        force_full: bool,
        transcript_dropped_events: u64,
    ) {
        let started_at = Instant::now();
        let vt_bytes_processed_since_last_snapshot =
            std::mem::replace(&mut self.pending_vt_bytes, 0);

        match self.refresh_inner(exit_status, force_full) {
            Ok(snapshot) => {
                state.publish_viewport(PublishedViewport {
                    viewport_snapshot: snapshot.viewport,
                    summary: snapshot.summary,
                    duration: started_at.elapsed(),
                    ghostty_dirty_state: snapshot.ghostty_dirty_state,
                    dirty_row_count: snapshot.dirty_row_count,
                    rendered_cell_count: snapshot.rendered_cell_count,
                    vt_bytes_processed_since_last_snapshot,
                    transcript_dropped_events,
                });
            }
            Err(error) => {
                state.set_error(&error, self.viewport_cache.geometry());
            }
        }
    }

    fn refresh_inner(
        &mut self,
        exit_status: Option<String>,
        force_full: bool,
    ) -> Result<RefreshSnapshot> {
        let snapshot = self
            .render_state
            .update(&self.terminal)
            .context("failed to update Ghostty render state")?;
        let dirty = match snapshot.dirty() {
            Ok(dirty) => dirty,
            Err(error)
                if error
                    .to_string()
                    .to_ascii_lowercase()
                    .contains("invalid value") =>
            {
                trace!(
                    error = %error,
                    "falling back to full redraw after invalid Ghostty dirty state"
                );
                Dirty::Full
            }
            Err(error) => return Err(error).context("failed to read Ghostty dirty state"),
        };
        let ghostty_dirty_state = match dirty {
            Dirty::Clean => crate::GhosttyDirtyState::Clean,
            Dirty::Partial => crate::GhosttyDirtyState::Partial,
            Dirty::Full => crate::GhosttyDirtyState::Full,
        };
        let colors = snapshot
            .colors()
            .context("failed to read Ghostty render colors")?;

        let mut rows = self
            .row_iterator
            .update(&snapshot)
            .context("failed to update Ghostty row iterator")?;
        let mut visible_rows = Vec::new();
        let mut dirty_row_count = 0;
        let mut rendered_cell_count = 0;
        let mut row_index = 0usize;
        let should_force_all = force_full || matches!(dirty, Dirty::Full);

        while let Some(row) = rows.next() {
            let row_dirty = should_force_all
                || matches!(dirty, Dirty::Partial)
                    && row
                        .dirty()
                        .context("failed to read Ghostty row dirty state")?;
            if row_dirty || row_index >= self.viewport_cache.row_count() {
                let rendered_row = render_row(
                    &mut self.cell_iterator,
                    row,
                    colors.foreground,
                    colors.background,
                )?;
                rendered_cell_count += rendered_row.cells.len();
                self.viewport_cache
                    .replace_row(row_index, rendered_row.clone());
                visible_rows.push(Arc::new(rendered_row));
                dirty_row_count += 1;
                row.set_dirty(false)
                    .context("failed to clear Ghostty row dirty state")?;
            } else if let Some(cached) = self.viewport_cache.row(row_index) {
                rendered_cell_count += cached.cells.len();
                visible_rows.push(Arc::clone(cached));
            }
            row_index += 1;
        }

        if row_index == 0 {
            if !should_force_all && dirty_row_count == 0 && self.viewport_cache.row_count() > 0 {
                rendered_cell_count = (0..self.viewport_cache.row_count())
                    .filter_map(|index| self.viewport_cache.row(index))
                    .map(|row| row.cells.len())
                    .sum();
            } else {
                let fallback = TerminalRow::default();
                self.viewport_cache.reset_rows(vec![fallback.clone()]);
                visible_rows.push(Arc::new(fallback));
                rendered_cell_count = 0;
                dirty_row_count = 1;
            }
        } else if should_force_all {
            let rows = visible_rows
                .iter()
                .map(|row| row.as_ref().clone())
                .collect::<Vec<_>>();
            self.viewport_cache.reset_rows(rows);
        }

        if dirty_row_count > 0 && !should_force_all {
            self.viewport_cache.bump_viewport_revision();
        }

        if matches!(dirty, Dirty::Clean) && row_index == 0 {
            let active_screen = if self.terminal.mode(Mode::ALT_SCREEN_SAVE).unwrap_or(false)
                || self.terminal.mode(Mode::ALT_SCREEN).unwrap_or(false)
                || self.terminal.mode(Mode::ALT_SCREEN_LEGACY).unwrap_or(false)
            {
                TerminalScreenKind::Alternate
            } else {
                TerminalScreenKind::Primary
            };
            if matches!(active_screen, TerminalScreenKind::Alternate) {
                self.viewport_cache.at_bottom = true;
            }
            let summary = self.viewport_cache.summary(
                exit_status,
                self.terminal.scrollback_rows().unwrap_or(0),
                active_screen,
                self.terminal.is_mouse_tracking().unwrap_or(false),
            );

            return Ok(RefreshSnapshot {
                viewport: self.viewport_cache.snapshot(),
                summary,
                ghostty_dirty_state,
                dirty_row_count,
                rendered_cell_count,
            });
        }

        let cursor = snapshot
            .cursor_viewport()
            .context("failed to query Ghostty cursor viewport")?
            .map(|cursor| TerminalCursor {
                x: cursor.x,
                y: cursor.y,
                at_wide_tail: cursor.at_wide_tail,
            });
        self.viewport_cache.set_cursor(cursor);
        snapshot
            .set_dirty(Dirty::Clean)
            .context("failed to clear Ghostty frame dirty state")?;

        let active_screen = if self.terminal.mode(Mode::ALT_SCREEN_SAVE).unwrap_or(false)
            || self.terminal.mode(Mode::ALT_SCREEN).unwrap_or(false)
            || self.terminal.mode(Mode::ALT_SCREEN_LEGACY).unwrap_or(false)
        {
            TerminalScreenKind::Alternate
        } else {
            TerminalScreenKind::Primary
        };
        if matches!(active_screen, TerminalScreenKind::Alternate) {
            self.viewport_cache.at_bottom = true;
        }
        let summary = self.viewport_cache.summary(
            exit_status,
            self.terminal.scrollback_rows().unwrap_or(0),
            active_screen,
            self.terminal.is_mouse_tracking().unwrap_or(false),
        );

        Ok(RefreshSnapshot {
            viewport: self.viewport_cache.snapshot(),
            summary,
            ghostty_dirty_state,
            dirty_row_count,
            rendered_cell_count,
        })
    }
}

struct RefreshSnapshot {
    viewport: crate::TerminalViewportSnapshot,
    summary: SessionSummary,
    ghostty_dirty_state: crate::GhosttyDirtyState,
    dirty_row_count: usize,
    rendered_cell_count: usize,
}

fn render_row(
    cell_iterator: &mut CellIterator<'static>,
    row: &libghostty_vt::render::RowIteration<'static, '_>,
    default_foreground: RgbColor,
    default_background: RgbColor,
) -> Result<TerminalRow> {
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
            ghostty_color_to_terminal(default_foreground),
            ghostty_color_to_terminal(default_background),
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

    Ok(rendered_row)
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
    use std::sync::Arc;

    use super::*;
    use crate::TerminalViewportSnapshot;

    fn new_test_emulator() -> (TerminalEmulator, SharedSessionState, TerminalGeometry) {
        let geometry = TerminalGeometry::default();
        let (state, _notify_rx) = SharedSessionState::new("init", geometry);
        let emulator = TerminalEmulator::new(geometry, "init").expect("terminal emulator");
        (emulator, state, geometry)
    }

    fn render_rows_from_vt(vt: &[u8]) -> Vec<TerminalRow> {
        let (mut emulator, state, _) = new_test_emulator();
        emulator.write(vt);
        emulator.refresh(&state, None, true, 0);

        state
            .viewport_snapshot()
            .rows
            .iter()
            .map(|row| row.as_ref().clone())
            .collect()
    }

    fn render_snapshot_from_vt(vt: &[u8]) -> (TerminalViewportSnapshot, SessionSummary) {
        let (mut emulator, state, _) = new_test_emulator();
        emulator.write(vt);
        emulator.refresh(&state, None, true, 0);
        (state.viewport_snapshot(), state.summary())
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
    fn invalid_partial_dirty_state_falls_back_to_full_redraw() {
        let (mut emulator, state, _) = new_test_emulator();
        emulator.write(b"alpha\x1b[2;1Hbeta\x1b[3;1Hgamma");
        emulator.refresh(&state, None, true, 0);
        let initial = state.viewport_snapshot();

        emulator.write(b"\x1b[HZETA");
        emulator.refresh(&state, None, false, 0);
        let updated = state.viewport_snapshot();
        let summary = state.summary();

        assert_ne!(
            summary.exit_status.as_deref(),
            Some("startup error"),
            "{}",
            summary.preview_line
        );
        assert_eq!(updated.row_revisions.len(), initial.row_revisions.len());
        assert!(
            initial
                .row_revisions
                .iter()
                .zip(updated.row_revisions.iter())
                .all(|(before, after)| before != after)
        );
    }

    #[test]
    fn partial_refresh_preserves_viewport_shape_and_reuses_unchanged_rows() {
        let (mut emulator, state, _) = new_test_emulator();
        emulator.write(b"alpha\x1b[2;1Hbeta\x1b[3;1Hgamma");
        emulator.refresh(&state, None, true, 0);
        let initial = state.viewport_snapshot();

        emulator.write(b"\x1b[HZETA");
        emulator.refresh(&state, None, false, 0);
        let updated = state.viewport_snapshot();
        let summary = state.summary();

        assert_ne!(
            summary.exit_status.as_deref(),
            Some("startup error"),
            "{}",
            summary.preview_line
        );
        assert_eq!(updated.row_count(), initial.row_count());
        assert_eq!(updated.row_revisions.len(), initial.row_revisions.len());
        assert!(!Arc::ptr_eq(&initial.rows[0], &updated.rows[0]));
    }

    #[test]
    fn force_full_refresh_rebuilds_all_rows() {
        let (mut emulator, state, _) = new_test_emulator();
        emulator.write(b"alpha\r\nbeta\r\ngamma");
        emulator.refresh(&state, None, true, 0);
        let initial = state.viewport_snapshot();

        emulator.refresh(&state, None, true, 0);
        let refreshed = state.viewport_snapshot();

        assert!(
            initial
                .rows
                .iter()
                .zip(refreshed.rows.iter())
                .all(|(before, after)| !Arc::ptr_eq(before, after))
        );
        assert!(
            initial
                .row_revisions
                .iter()
                .zip(refreshed.row_revisions.iter())
                .all(|(before, after)| before != after)
        );
    }

    #[test]
    fn refresh_summary_tracks_scrollback_and_preview() {
        let (viewport, summary) = render_snapshot_from_vt(b"alpha\r\nbeta\r\n");

        assert!(viewport.row_count() >= 2);
        assert_eq!(summary.preview_line.trim_end(), "beta");
        assert!(summary.at_bottom);
    }
}
