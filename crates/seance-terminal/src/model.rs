use std::sync::Arc;

use anyhow::{Result, ensure};

const DEFAULT_COLS: u16 = 120;
const DEFAULT_ROWS: u16 = 36;
const DEFAULT_CELL_WIDTH_PX: u16 = 8;
const DEFAULT_CELL_HEIGHT_PX: u16 = 19;
const DEFAULT_PIXEL_WIDTH: u16 = DEFAULT_COLS * DEFAULT_CELL_WIDTH_PX;
const DEFAULT_PIXEL_HEIGHT: u16 = DEFAULT_ROWS * DEFAULT_CELL_HEIGHT_PX;

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
        ensure!(cols > 0, "terminal cols must be greater than zero");
        ensure!(rows > 0, "terminal rows must be greater than zero");
        ensure!(
            cell_width_px > 0,
            "terminal cell width must be greater than zero"
        );
        ensure!(
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalCursor {
    pub x: u16,
    pub y: u16,
    pub at_wide_tail: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalCursorVisualStyle {
    Bar,
    #[default]
    Block,
    Underline,
    BlockHollow,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalCursorState {
    pub position: TerminalCursor,
    pub visual_style: TerminalCursorVisualStyle,
    pub visible: bool,
    pub blinking: bool,
    pub color: Option<TerminalColor>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalScrollbarState {
    pub total_rows: u64,
    pub offset_rows: u64,
    pub visible_rows: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TerminalScreenKind {
    #[default]
    Primary,
    Alternate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalScrollCommand {
    Top,
    Bottom,
    DeltaRows(isize),
    SetOffsetRows(u64),
    PageUp,
    PageDown,
}

#[derive(Clone, Debug, Default)]
pub struct SessionSummary {
    pub exit_status: Option<String>,
    pub preview_line: String,
    pub viewport_revision: u64,
    pub scrollback_rows: usize,
    pub active_screen: TerminalScreenKind,
    pub mouse_tracking: bool,
    pub at_bottom: bool,
}

#[derive(Clone, Debug, Default)]
pub struct TerminalViewportSnapshot {
    pub rows: Arc<[Arc<TerminalRow>]>,
    pub row_revisions: Arc<[u64]>,
    pub cursor: Option<TerminalCursorState>,
    pub scrollbar: Option<TerminalScrollbarState>,
    pub revision: u64,
    pub cols: u16,
    pub rows_visible: u16,
}

impl TerminalViewportSnapshot {
    pub fn row_count(&self) -> usize {
        self.rows.len()
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
}
