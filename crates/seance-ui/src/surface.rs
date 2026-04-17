use std::{collections::HashMap, ops::Range, sync::Arc};

use gpui::{Pixels, ShapedLine};
use seance_terminal::{
    TerminalCellStyle, TerminalCursorState, TerminalGeometry, TerminalScrollbarState,
};

use crate::{
    TerminalMetrics, TerminalRendererMetrics,
    model::{TerminalHoveredLink, TerminalSelection},
    theme::ThemeId,
};

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
    pub(crate) link_highlights: Vec<TerminalPaintQuad>,
    pub(crate) underlines: Vec<TerminalPaintQuad>,
    pub(crate) link_underlines: Vec<TerminalPaintQuad>,
    pub(crate) link_ranges: Vec<Range<usize>>,
    pub(crate) fragments: Vec<TerminalPaintFragment>,
}

#[derive(Clone, Debug)]
pub(crate) struct TerminalSurfaceState {
    pub(crate) active_session_id: u64,
    pub(crate) viewport_revision: u64,
    pub(crate) row_revisions: Vec<u64>,
    pub(crate) geometry: Option<TerminalGeometry>,
    pub(crate) theme_id: ThemeId,
    pub(crate) cursor: Option<TerminalCursorState>,
    pub(crate) scrollbar: Option<TerminalScrollbarState>,
    pub(crate) selection: Option<TerminalSelection>,
    pub(crate) rows: Arc<[TerminalPaintRow]>,
    pub(crate) metrics: TerminalRendererMetrics,
    pub(crate) shape_cache: ShapeCache,
}

impl Default for TerminalSurfaceState {
    fn default() -> Self {
        Self {
            active_session_id: 0,
            viewport_revision: 0,
            row_revisions: Vec::new(),
            geometry: None,
            theme_id: ThemeId::ObsidianSmoke,
            cursor: None,
            scrollbar: None,
            selection: None,
            rows: Arc::from(Vec::<TerminalPaintRow>::new()),
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
    pub(crate) rows: Arc<[TerminalPaintRow]>,
    pub(crate) metrics: TerminalMetrics,
    pub(crate) cursor: Option<TerminalCursorState>,
    pub(crate) selection: Option<TerminalSelection>,
    pub(crate) hovered_link: Option<TerminalHoveredLink>,
    pub(crate) terminal_focused: bool,
    pub(crate) cursor_fallback: gpui::Hsla,
    pub(crate) cursor_dim: gpui::Hsla,
    pub(crate) selection_background: gpui::Hsla,
    pub(crate) link_hover_background: gpui::Hsla,
    pub(crate) link_hover_underline: gpui::Hsla,
    pub(crate) link_modifier_background: gpui::Hsla,
    pub(crate) link_modifier_underline: gpui::Hsla,
}
