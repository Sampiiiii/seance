use std::{num::NonZeroUsize, ops::Range, sync::Arc};

use gpui::{Pixels, ShapedLine};
use lru::LruCache;
use seance_terminal::{
    TerminalCellStyle, TerminalCursorState, TerminalGeometry, TerminalScrollbarState,
};

use crate::{
    TerminalMetrics, TerminalRendererMetrics,
    model::{TerminalHoveredLink, TerminalSelection},
    theme::ThemeId,
};

pub(crate) const SHAPE_CACHE_CAPACITY: usize = 2_048;
pub(crate) const ROW_TEMPLATE_CACHE_CAPACITY: usize = 4_096;

fn shape_cache_capacity() -> NonZeroUsize {
    NonZeroUsize::new(SHAPE_CACHE_CAPACITY).expect("shape cache capacity must be non-zero")
}

fn row_template_cache_capacity() -> NonZeroUsize {
    NonZeroUsize::new(ROW_TEMPLATE_CACHE_CAPACITY)
        .expect("row template cache capacity must be non-zero")
}

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

#[derive(Clone, Debug)]
pub(crate) struct TerminalPaintRow {
    pub(crate) y: Pixels,
    pub(crate) backgrounds: Arc<[TerminalPaintQuad]>,
    pub(crate) link_highlights: Arc<[TerminalPaintQuad]>,
    pub(crate) underlines: Arc<[TerminalPaintQuad]>,
    pub(crate) link_underlines: Arc<[TerminalPaintQuad]>,
    pub(crate) link_ranges: Arc<[Range<usize>]>,
    pub(crate) fragments: Arc<[TerminalPaintFragment]>,
}

impl Default for TerminalPaintRow {
    fn default() -> Self {
        Self {
            y: Pixels::default(),
            backgrounds: Arc::from(Vec::<TerminalPaintQuad>::new()),
            link_highlights: Arc::from(Vec::<TerminalPaintQuad>::new()),
            underlines: Arc::from(Vec::<TerminalPaintQuad>::new()),
            link_underlines: Arc::from(Vec::<TerminalPaintQuad>::new()),
            link_ranges: Arc::from(Vec::<Range<usize>>::new()),
            fragments: Arc::from(Vec::<TerminalPaintFragment>::new()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct RowPaintTemplate {
    pub(crate) backgrounds: Arc<[TerminalPaintQuad]>,
    pub(crate) link_highlights: Arc<[TerminalPaintQuad]>,
    pub(crate) underlines: Arc<[TerminalPaintQuad]>,
    pub(crate) link_underlines: Arc<[TerminalPaintQuad]>,
    pub(crate) link_ranges: Arc<[Range<usize>]>,
    pub(crate) fragments: Arc<[TerminalPaintFragment]>,
}

impl Default for RowPaintTemplate {
    fn default() -> Self {
        Self {
            backgrounds: Arc::from(Vec::<TerminalPaintQuad>::new()),
            link_highlights: Arc::from(Vec::<TerminalPaintQuad>::new()),
            underlines: Arc::from(Vec::<TerminalPaintQuad>::new()),
            link_underlines: Arc::from(Vec::<TerminalPaintQuad>::new()),
            link_ranges: Arc::from(Vec::<Range<usize>>::new()),
            fragments: Arc::from(Vec::<TerminalPaintFragment>::new()),
        }
    }
}

impl RowPaintTemplate {
    pub(crate) fn materialize(&self, row_index: usize, line_height_px: f32) -> TerminalPaintRow {
        TerminalPaintRow {
            y: gpui::px(row_index as f32 * line_height_px),
            backgrounds: Arc::clone(&self.backgrounds),
            link_highlights: Arc::clone(&self.link_highlights),
            underlines: Arc::clone(&self.underlines),
            link_underlines: Arc::clone(&self.link_underlines),
            link_ranges: Arc::clone(&self.link_ranges),
            fragments: Arc::clone(&self.fragments),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum LinkPaintMode {
    #[default]
    Normal,
    Deferred,
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
    pub(crate) rows_scratch: Vec<TerminalPaintRow>,
    pub(crate) metrics: TerminalRendererMetrics,
    /// Hash of the `TerminalMetrics` used for the last full rebuild. When the
    /// live metrics (font size, DPI-driven cell dimensions) change but the
    /// geometry stays identical, this fingerprint diverging forces a full
    /// rebuild so we don't paint rows at stale y-positions.
    pub(crate) metrics_fingerprint: u64,
    pub(crate) shape_cache: ShapeCache,
    pub(crate) row_template_cache: RowPaintCache,
    pub(crate) link_paint_mode: LinkPaintMode,
}

impl TerminalSurfaceState {
    /// Clears every field that the partial-rebuild branch in
    /// `sync_terminal_surface` keys off of, so the next sync is guaranteed to
    /// enter the full-rebuild branch. This is the core invariant behind
    /// `SeanceWorkspace::invalidate_terminal_surface`.
    pub(crate) fn mark_invalidated(&mut self) {
        self.viewport_revision = 0;
        self.row_revisions.clear();
        self.geometry = None;
        self.rows = Arc::from(Vec::<TerminalPaintRow>::new());
        self.rows_scratch.clear();
        self.metrics = TerminalRendererMetrics::default();
        self.metrics_fingerprint = 0;
    }
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
            rows_scratch: Vec::new(),
            metrics: TerminalRendererMetrics::default(),
            metrics_fingerprint: 0,
            shape_cache: ShapeCache::default(),
            row_template_cache: RowPaintCache::default(),
            link_paint_mode: LinkPaintMode::Normal,
        }
    }
}

#[derive(Debug)]
pub(crate) struct ShapeCache {
    pub(crate) entries: LruCache<ShapeCacheKey, CachedShapeLine>,
}

impl Default for ShapeCache {
    fn default() -> Self {
        Self {
            entries: LruCache::new(shape_cache_capacity()),
        }
    }
}

impl Clone for ShapeCache {
    fn clone(&self) -> Self {
        let mut cloned = LruCache::new(
            NonZeroUsize::new(self.entries.cap().get()).unwrap_or_else(shape_cache_capacity),
        );
        for (key, entry) in self.entries.iter().rev() {
            cloned.put(key.clone(), entry.clone());
        }
        Self { entries: cloned }
    }
}

#[derive(Debug)]
pub(crate) struct RowPaintCache {
    pub(crate) entries: LruCache<RowPaintCacheKey, CachedRowPaintTemplate>,
}

impl Default for RowPaintCache {
    fn default() -> Self {
        Self {
            entries: LruCache::new(row_template_cache_capacity()),
        }
    }
}

impl Clone for RowPaintCache {
    fn clone(&self) -> Self {
        let mut cloned = LruCache::new(
            NonZeroUsize::new(self.entries.cap().get())
                .unwrap_or_else(row_template_cache_capacity),
        );
        for (key, entry) in self.entries.iter().rev() {
            cloned.put(key.clone(), entry.clone());
        }
        Self { entries: cloned }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CachedRowPaintTemplate {
    pub(crate) template: RowPaintTemplate,
}

#[derive(Clone, Debug)]
pub(crate) struct CachedShapeLine {
    pub(crate) line: ShapedLine,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ShapeCacheKey {
    pub(crate) text: Arc<str>,
    pub(crate) font_family: Arc<str>,
    pub(crate) font_size_bits: u32,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) color: HslaKey,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RowPaintCacheKey {
    pub(crate) content_style_hash: u64,
    pub(crate) visible_cols: usize,
    pub(crate) font_family: Arc<str>,
    pub(crate) font_size_bits: u32,
    pub(crate) cell_width_bits: u32,
    pub(crate) line_height_bits: u32,
    pub(crate) cell_height_bits: u32,
    pub(crate) theme_id: ThemeId,
    pub(crate) link_paint_mode: LinkPaintMode,
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

#[cfg(test)]
mod tests {
    use super::*;
    use seance_terminal::{TerminalGeometry, TerminalSize};

    #[test]
    fn mark_invalidated_clears_every_fast_path_field() {
        let mut state = TerminalSurfaceState::default();
        state.viewport_revision = 42;
        state.row_revisions = vec![1, 2, 3];
        state.geometry = Some(TerminalGeometry {
            size: TerminalSize { rows: 24, cols: 80 },
            ..TerminalGeometry::default()
        });
        state.rows = Arc::from(vec![TerminalPaintRow::default(); 3]);
        state.rows_scratch = vec![TerminalPaintRow::default(); 3];
        state.metrics = TerminalRendererMetrics {
            visible_rows: 24,
            ..Default::default()
        };
        state.metrics_fingerprint = 0xdead_beef_u64;

        state.mark_invalidated();

        assert_eq!(state.viewport_revision, 0);
        assert!(state.row_revisions.is_empty());
        assert_eq!(state.geometry, None);
        assert!(state.rows.is_empty());
        assert!(state.rows_scratch.is_empty());
        assert_eq!(state.metrics, TerminalRendererMetrics::default());
        assert_eq!(state.metrics_fingerprint, 0);
    }

    #[test]
    fn mark_invalidated_on_default_state_is_idempotent() {
        let mut state = TerminalSurfaceState::default();
        state.mark_invalidated();
        assert!(state.rows.is_empty());
        assert!(state.rows_scratch.is_empty());
        assert_eq!(state.metrics_fingerprint, 0);
    }
}
