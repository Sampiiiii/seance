use std::collections::HashMap;

use gpui::{Pixels, ShapedLine};
use seance_terminal::{TerminalCellStyle, TerminalGeometry};

use crate::{TerminalRendererMetrics, theme::ThemeId};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TerminalGlyphPolicy {
    GroupableAscii,
    PerCellSpecial,
    WideCell,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalPaintFragment {
    pub(crate) x: Pixels,
    pub(crate) line: ShapedLine,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct TerminalPaintQuad {
    pub(crate) x: Pixels,
    pub(crate) width: Pixels,
    pub(crate) color: gpui::Hsla,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TerminalPaintRow {
    pub(crate) y: Pixels,
    pub(crate) backgrounds: Vec<TerminalPaintQuad>,
    pub(crate) underlines: Vec<TerminalPaintQuad>,
    pub(crate) fragments: Vec<TerminalPaintFragment>,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalSurfaceState {
    pub(crate) active_session_id: u64,
    pub(crate) snapshot_seq: u64,
    pub(crate) geometry: Option<TerminalGeometry>,
    pub(crate) theme_id: ThemeId,
    pub(crate) rows: Vec<TerminalPaintRow>,
    pub(crate) metrics: TerminalRendererMetrics,
    pub(crate) shape_cache: ShapeCache,
}

impl Default for TerminalSurfaceState {
    fn default() -> Self {
        Self {
            active_session_id: 0,
            snapshot_seq: 0,
            geometry: None,
            theme_id: ThemeId::ObsidianSmoke,
            rows: Vec::new(),
            metrics: TerminalRendererMetrics::default(),
            shape_cache: ShapeCache::default(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ShapeCache {
    pub(crate) entries: HashMap<ShapeCacheKey, CachedShapeLine>,
    pub(crate) generation: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct CachedShapeLine {
    pub(crate) line: ShapedLine,
    pub(crate) last_used: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ShapeCacheKey {
    pub(crate) text: String,
    pub(crate) font_family: String,
    pub(crate) font_size_bits: u32,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) color: HslaKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct HslaKey {
    pub(crate) h: u32,
    pub(crate) s: u32,
    pub(crate) l: u32,
    pub(crate) a: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TerminalFragmentPlan {
    pub(crate) text: String,
    pub(crate) style: TerminalCellStyle,
    pub(crate) glyph_policy: TerminalGlyphPolicy,
    pub(crate) start_col: usize,
    pub(crate) cell_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedTerminalSurface {
    pub(crate) rows: Vec<TerminalPaintRow>,
    pub(crate) line_height_px: f32,
}
