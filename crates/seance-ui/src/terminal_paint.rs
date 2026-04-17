use gpui::{App, Bounds, Pixels, SharedString, TextRun, Window, fill, font, point, px, size};
use seance_terminal::{
    TerminalCell, TerminalCellStyle, TerminalColor, TerminalCursorState, TerminalCursorVisualStyle,
    TerminalRow,
};

use crate::{
    CachedShapeLine, HslaKey, PreparedTerminalSurface, ShapeCache, ShapeCacheKey,
    TerminalFragmentPlan, TerminalGlyphPolicy, TerminalMetrics, TerminalPaintFragment,
    TerminalPaintQuad, TerminalPaintRow, TerminalRendererMetrics, ThemeId,
    model::{TerminalHoveredLink, TerminalSelection, TerminalSelectionPoint},
    terminal_links::terminal_links_for_row,
    theme::Theme,
};

const TERMINAL_LINK_LEFT_INSET_PX: f32 = 1.0;
const TERMINAL_LINK_RIGHT_INSET_PX: f32 = 5.0;
const WIDTH_DRIFT_THRESHOLD_PX: f32 = 0.25;

#[derive(Clone)]
pub(crate) struct ShapedTerminalFragment {
    pub(crate) line: gpui::ShapedLine,
    pub(crate) width_error_px: f32,
}

pub(crate) fn build_terminal_paint_row(
    row: &TerminalRow,
    row_index: usize,
    visible_cols: usize,
    metrics: TerminalMetrics,
    theme_id: ThemeId,
    theme: &Theme,
    font_family: &str,
    shape_cache: &mut ShapeCache,
    window: &mut Window,
    renderer_metrics: &mut TerminalRendererMetrics,
) -> TerminalPaintRow {
    let fragment_plans = terminal_fragment_plans(row, visible_cols, theme, renderer_metrics);
    let backgrounds = terminal_background_quads(row, visible_cols, metrics, theme);
    let link_paint = terminal_link_paint(row, visible_cols, metrics, theme);
    let underlines = terminal_underline_quads(row, visible_cols, metrics, theme);
    let mut fragments = Vec::with_capacity(fragment_plans.len());

    for plan in fragment_plans {
        if plan.text.is_empty() {
            continue;
        }
        let shaped = shape_terminal_fragment(
            &plan,
            metrics,
            theme_id,
            theme,
            font_family,
            shape_cache,
            window,
            renderer_metrics,
        );
        if should_cell_align_fragment(&plan, shaped.width_error_px) {
            renderer_metrics.cell_aligned_fallback_fragments = renderer_metrics
                .cell_aligned_fallback_fragments
                .saturating_add(1);
            for (text, x) in cell_aligned_ascii_cells(&plan, metrics.cell_width_px) {
                let cell_plan = TerminalFragmentPlan {
                    text,
                    style: plan.style,
                    glyph_policy: plan.glyph_policy,
                    start_col: plan.start_col,
                    cell_count: 1,
                };
                let aligned = shape_terminal_fragment(
                    &cell_plan,
                    metrics,
                    theme_id,
                    theme,
                    font_family,
                    shape_cache,
                    window,
                    renderer_metrics,
                );
                fragments.push(TerminalPaintFragment {
                    x,
                    line: aligned.line,
                });
            }
        } else {
            fragments.push(TerminalPaintFragment {
                x: px(plan.start_col as f32 * metrics.cell_width_px),
                line: shaped.line,
            });
        }
    }

    renderer_metrics.fragments += fragments.len();
    renderer_metrics.background_quads += backgrounds.len()
        + link_paint.highlights.len()
        + underlines.len()
        + link_paint.underlines.len();

    TerminalPaintRow {
        y: px(row_index as f32 * metrics.line_height_px),
        backgrounds,
        link_highlights: link_paint.highlights,
        underlines,
        link_underlines: link_paint.underlines,
        link_ranges: link_paint.ranges,
        fragments,
    }
}

pub(crate) fn terminal_fragment_plans(
    row: &TerminalRow,
    visible_cols: usize,
    theme: &Theme,
    renderer_metrics: &mut TerminalRendererMetrics,
) -> Vec<TerminalFragmentPlan> {
    let mut plans = Vec::new();
    let mut current_col = 0;
    let mut current: Option<TerminalFragmentPlan> = None;

    for cell in &row.cells {
        if current_col >= visible_cols {
            break;
        }

        let cell_width = usize::from(cell.width.max(1));
        if current_col + cell_width > visible_cols {
            break;
        }

        let glyph_policy = terminal_glyph_policy(cell);
        if matches!(glyph_policy, TerminalGlyphPolicy::PerCellSpecial) {
            renderer_metrics.special_glyph_cells += cell_width;
        }
        if matches!(glyph_policy, TerminalGlyphPolicy::WideCell) {
            renderer_metrics.wide_cells += 1;
        }

        let is_blank = cell.text.chars().all(|ch| ch == ' ');
        if is_blank {
            if let Some(plan) = current.take() {
                plans.push(plan);
            }
            current_col += cell_width;
            continue;
        }

        let should_merge = current.as_ref().is_some_and(|plan| {
            plan.style == cell.style
                && plan.glyph_policy == glyph_policy
                && plan.start_col + plan.cell_count == current_col
                && glyph_policy == TerminalGlyphPolicy::GroupableAscii
        });

        if should_merge {
            let plan = current.as_mut().expect("current fragment exists");
            plan.text.push_str(&cell.text);
            plan.cell_count += cell_width;
        } else {
            if let Some(plan) = current.take() {
                plans.push(plan);
            }
            current = Some(TerminalFragmentPlan {
                text: cell.text.clone(),
                style: cell.style,
                glyph_policy,
                start_col: current_col,
                cell_count: cell_width,
            });
        }

        current_col += cell_width;
    }

    if let Some(plan) = current.take() {
        plans.push(plan);
    }

    let _ = theme;
    plans
}

pub(crate) fn terminal_background_quads(
    row: &TerminalRow,
    visible_cols: usize,
    metrics: TerminalMetrics,
    theme: &Theme,
) -> Vec<TerminalPaintQuad> {
    let mut quads = Vec::new();
    let mut current_col = 0;
    let mut run_start = 0;
    let mut run_width = 0;
    let mut run_color: Option<gpui::Hsla> = None;

    for cell in &row.cells {
        if current_col >= visible_cols {
            break;
        }

        let cell_width = usize::from(cell.width.max(1));
        if current_col + cell_width > visible_cols {
            break;
        }

        let cell_color = cell.style.background.map(terminal_color_to_hsla);
        if cell_color == run_color {
            run_width += cell_width;
        } else {
            if let Some(color) = run_color {
                quads.push(TerminalPaintQuad {
                    x: px(run_start as f32 * metrics.cell_width_px),
                    width: px(run_width as f32 * metrics.cell_width_px),
                    color,
                });
            }
            run_start = current_col;
            run_width = cell_width;
            run_color = cell_color;
        }

        current_col += cell_width;
    }

    if let Some(color) = run_color {
        quads.push(TerminalPaintQuad {
            x: px(run_start as f32 * metrics.cell_width_px),
            width: px(run_width as f32 * metrics.cell_width_px),
            color,
        });
    }

    let _ = theme;
    quads
}

pub(crate) fn terminal_underline_quads(
    row: &TerminalRow,
    visible_cols: usize,
    metrics: TerminalMetrics,
    theme: &Theme,
) -> Vec<TerminalPaintQuad> {
    let mut quads = Vec::new();
    let mut current_col = 0;
    let mut run_start = 0;
    let mut run_width = 0;
    let mut run_color: Option<gpui::Hsla> = None;

    for cell in &row.cells {
        if current_col >= visible_cols {
            break;
        }

        let cell_width = usize::from(cell.width.max(1));
        if current_col + cell_width > visible_cols {
            break;
        }

        let cell_color = cell
            .style
            .underline
            .then(|| resolve_terminal_foreground(cell.style, theme));
        if cell_color == run_color {
            run_width += cell_width;
        } else {
            if let Some(color) = run_color {
                quads.push(TerminalPaintQuad {
                    x: px(run_start as f32 * metrics.cell_width_px),
                    width: px(run_width as f32 * metrics.cell_width_px),
                    color,
                });
            }
            run_start = current_col;
            run_width = cell_width;
            run_color = cell_color;
        }

        current_col += cell_width;
    }

    if let Some(color) = run_color {
        quads.push(TerminalPaintQuad {
            x: px(run_start as f32 * metrics.cell_width_px),
            width: px(run_width as f32 * metrics.cell_width_px),
            color,
        });
    }

    quads
}

#[derive(Debug)]
struct TerminalLinkPaint {
    ranges: Vec<std::ops::Range<usize>>,
    highlights: Vec<TerminalPaintQuad>,
    underlines: Vec<TerminalPaintQuad>,
}

fn terminal_link_paint(
    row: &TerminalRow,
    visible_cols: usize,
    metrics: TerminalMetrics,
    theme: &Theme,
) -> TerminalLinkPaint {
    let links = terminal_links_for_row(row, visible_cols);
    let mut ranges = Vec::with_capacity(links.len());
    let mut highlights = Vec::with_capacity(links.len());
    let mut underlines = Vec::with_capacity(links.len());

    for link in links {
        let start_col = link.col_range.start;
        let end_col = link.col_range.end;
        if start_col >= end_col {
            continue;
        }
        let x = px(start_col as f32 * metrics.cell_width_px);
        let width = px((end_col - start_col) as f32 * metrics.cell_width_px);
        if width <= px(0.0) {
            continue;
        }

        ranges.push(link.col_range);
        highlights.push(TerminalPaintQuad {
            x,
            width,
            color: theme.terminal_link,
        });
        underlines.push(TerminalPaintQuad {
            x,
            width,
            color: theme.terminal_link_underline,
        });
    }

    TerminalLinkPaint {
        ranges,
        highlights,
        underlines,
    }
}

pub(crate) fn shape_terminal_fragment(
    plan: &TerminalFragmentPlan,
    metrics: TerminalMetrics,
    theme_id: ThemeId,
    theme: &Theme,
    font_family: &str,
    shape_cache: &mut ShapeCache,
    window: &mut Window,
    renderer_metrics: &mut TerminalRendererMetrics,
) -> ShapedTerminalFragment {
    let color = resolve_terminal_foreground(plan.style, theme);
    let key = ShapeCacheKey {
        text: plan.text.clone(),
        font_family: font_family.to_string(),
        font_size_bits: metrics.font_size_px.to_bits(),
        bold: plan.style.bold,
        italic: plan.style.italic,
        color: hsla_key(color),
    };

    if let Some(entry) = shape_cache.entries.get_mut(&key) {
        shape_cache.generation = shape_cache.generation.saturating_add(1);
        entry.last_used = shape_cache.generation;
        renderer_metrics.shape_hits += 1;
        return shaped_fragment_with_metrics(entry.line.clone(), plan, metrics, renderer_metrics);
    }

    renderer_metrics.shape_misses += 1;
    let mut terminal_font = font(font_family.to_string());
    if plan.style.bold {
        terminal_font = terminal_font.bold();
    }
    if plan.style.italic {
        terminal_font = terminal_font.italic();
    }

    let line = window.text_system().shape_line(
        SharedString::from(plan.text.clone()),
        px(metrics.font_size_px),
        &[TextRun {
            len: plan.text.len(),
            font: terminal_font,
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        None,
    );

    shape_cache.generation = shape_cache.generation.saturating_add(1);
    shape_cache.entries.insert(
        key,
        CachedShapeLine {
            line: line.clone(),
            last_used: shape_cache.generation,
        },
    );
    evict_shape_cache(shape_cache, 2_048);
    let _ = theme_id;
    shaped_fragment_with_metrics(line, plan, metrics, renderer_metrics)
}

fn shaped_fragment_with_metrics(
    line: gpui::ShapedLine,
    plan: &TerminalFragmentPlan,
    metrics: TerminalMetrics,
    renderer_metrics: &mut TerminalRendererMetrics,
) -> ShapedTerminalFragment {
    let expected_width_px = plan.cell_count as f32 * metrics.cell_width_px;
    let actual_width_px = f32::from(line.width);
    let width_error_px = (actual_width_px - expected_width_px).abs();
    let width_error_milli_px = width_error_milli_px(width_error_px);

    renderer_metrics.total_width_error_milli_px = renderer_metrics
        .total_width_error_milli_px
        .saturating_add(u64::from(width_error_milli_px));
    renderer_metrics.max_width_error_milli_px = renderer_metrics
        .max_width_error_milli_px
        .max(width_error_milli_px);
    if width_error_px > WIDTH_DRIFT_THRESHOLD_PX {
        renderer_metrics.width_mismatch_fragments =
            renderer_metrics.width_mismatch_fragments.saturating_add(1);
    }

    ShapedTerminalFragment {
        line,
        width_error_px,
    }
}

fn width_error_milli_px(width_error_px: f32) -> u32 {
    if !width_error_px.is_finite() {
        return 0;
    }
    let milli = (width_error_px.max(0.0) * 1_000.0).round();
    if milli >= u32::MAX as f32 {
        u32::MAX
    } else {
        milli as u32
    }
}

fn should_cell_align_fragment(plan: &TerminalFragmentPlan, width_error_px: f32) -> bool {
    plan.glyph_policy == TerminalGlyphPolicy::GroupableAscii
        && width_error_px > WIDTH_DRIFT_THRESHOLD_PX
}

fn cell_aligned_ascii_cells(
    plan: &TerminalFragmentPlan,
    cell_width_px: f32,
) -> Vec<(String, Pixels)> {
    plan.text
        .chars()
        .take(plan.cell_count)
        .enumerate()
        .map(|(offset, ch)| {
            (
                ch.to_string(),
                px((plan.start_col + offset) as f32 * cell_width_px),
            )
        })
        .collect()
}

pub(crate) fn evict_shape_cache(shape_cache: &mut ShapeCache, limit: usize) {
    if shape_cache.entries.len() <= limit {
        return;
    }

    if let Some((oldest_key, _)) = shape_cache
        .entries
        .iter()
        .min_by_key(|(_, entry)| entry.last_used)
        .map(|(key, entry)| (key.clone(), entry.last_used))
    {
        shape_cache.entries.remove(&oldest_key);
    }
}

pub(crate) fn hsla_key(color: gpui::Hsla) -> HslaKey {
    HslaKey {
        h: color.h.to_bits(),
        s: color.s.to_bits(),
        l: color.l.to_bits(),
        a: color.a.to_bits(),
    }
}

pub(crate) fn paint_terminal_surface(
    bounds: Bounds<Pixels>,
    surface: PreparedTerminalSurface,
    window: &mut Window,
    cx: &mut App,
) {
    let line_height = px(surface.metrics.line_height_px);
    let visible_cols = (f32::from(bounds.size.width) / surface.metrics.cell_width_px)
        .floor()
        .max(0.0) as usize;

    for row in surface.rows.iter() {
        let row_origin = point(bounds.origin.x, bounds.origin.y + row.y);
        let row_index = ((f32::from(row.y) / surface.metrics.line_height_px).round() as usize)
            .min(surface.rows.len().saturating_sub(1));

        if let Some(selection) = surface.selection
            && let Some((start_col, end_col)) =
                selection_span_for_row(selection, row_index, visible_cols)
        {
            let start_x = row_origin.x + px(start_col as f32 * surface.metrics.cell_width_px);
            let width =
                px((end_col.saturating_sub(start_col)) as f32 * surface.metrics.cell_width_px);
            if width > px(0.0) {
                window.paint_quad(fill(
                    Bounds::new(point(start_x, row_origin.y), size(width, line_height)),
                    surface.selection_background,
                ));
            }
        }

        for background in &row.backgrounds {
            window.paint_quad(fill(
                Bounds::new(
                    point(row_origin.x + background.x, row_origin.y),
                    size(background.width, line_height),
                ),
                background.color,
            ));
        }

        for highlight in &row.link_highlights {
            paint_terminal_link_background_quad(row_origin, line_height, *highlight, window);
        }

        if let Some(hovered_link) = surface
            .hovered_link
            .as_ref()
            .filter(|hovered_link| hovered_link.row == row_index)
            .filter(|hovered_link| {
                row.link_ranges
                    .iter()
                    .any(|range| range == &hovered_link.col_range)
            })
        {
            paint_hovered_terminal_link(row_origin, hovered_link, line_height, &surface, window);
        }

        for fragment in &row.fragments {
            let _ = fragment.line.paint(
                point(row_origin.x + fragment.x, row_origin.y),
                line_height,
                window,
                cx,
            );
        }

        for underline in &row.underlines {
            window.paint_quad(fill(
                Bounds::new(
                    point(
                        row_origin.x + underline.x,
                        row_origin.y + line_height - px(1.0),
                    ),
                    size(underline.width, px(1.0)),
                ),
                underline.color,
            ));
        }

        for underline in &row.link_underlines {
            paint_terminal_underline_quad(row_origin, line_height, *underline, px(1.0), window);
        }
    }

    if let Some(cursor) = surface.cursor
        && cursor.visible
    {
        paint_terminal_cursor(bounds, cursor, &surface, window);
    }
}

fn paint_terminal_cursor(
    bounds: Bounds<Pixels>,
    cursor: TerminalCursorState,
    surface: &PreparedTerminalSurface,
    window: &mut Window,
) {
    let metrics = surface.metrics;
    let cursor_col = if cursor.position.at_wide_tail {
        cursor.position.x.saturating_sub(1)
    } else {
        cursor.position.x
    };
    let x = bounds.origin.x + px(f32::from(cursor_col) * metrics.cell_width_px);
    let y = bounds.origin.y + px(f32::from(cursor.position.y) * metrics.line_height_px);
    let cell_width = px(metrics.cell_width_px.max(1.0));
    let line_height = px(metrics.line_height_px.max(1.0));
    let color = cursor
        .color
        .map(terminal_color_to_hsla)
        .unwrap_or(if surface.terminal_focused {
            surface.cursor_fallback
        } else {
            surface.cursor_dim
        });

    match cursor.visual_style {
        TerminalCursorVisualStyle::Bar => {
            window.paint_quad(fill(
                Bounds::new(point(x, y), size(px(2.0), line_height)),
                color,
            ));
        }
        TerminalCursorVisualStyle::Underline => {
            window.paint_quad(fill(
                Bounds::new(
                    point(x, y + line_height - px(2.0)),
                    size(cell_width, px(2.0)),
                ),
                color,
            ));
        }
        TerminalCursorVisualStyle::Block => {
            window.paint_quad(fill(
                Bounds::new(point(x, y), size(cell_width, line_height)),
                color.alpha(if surface.terminal_focused { 0.45 } else { 0.22 }),
            ));
        }
        TerminalCursorVisualStyle::BlockHollow => {
            window.paint_quad(fill(
                Bounds::new(point(x, y), size(cell_width, px(1.0))),
                color,
            ));
            window.paint_quad(fill(
                Bounds::new(
                    point(x, y + line_height - px(1.0)),
                    size(cell_width, px(1.0)),
                ),
                color,
            ));
            window.paint_quad(fill(
                Bounds::new(point(x, y), size(px(1.0), line_height)),
                color,
            ));
            window.paint_quad(fill(
                Bounds::new(
                    point(x + cell_width - px(1.0), y),
                    size(px(1.0), line_height),
                ),
                color,
            ));
        }
    }
}

fn paint_hovered_terminal_link(
    row_origin: gpui::Point<Pixels>,
    hovered_link: &TerminalHoveredLink,
    line_height: Pixels,
    surface: &PreparedTerminalSurface,
    window: &mut Window,
) {
    let x = row_origin.x + px(hovered_link.col_range.start as f32 * surface.metrics.cell_width_px);
    let width = px(hovered_link.col_range.len() as f32 * surface.metrics.cell_width_px);
    if width <= px(0.0) {
        return;
    }

    let (background, underline, thickness) = if hovered_link.modifier_active {
        (
            surface.link_modifier_background,
            surface.link_modifier_underline,
            px(2.0),
        )
    } else {
        (
            surface.link_hover_background,
            surface.link_hover_underline,
            px(1.5),
        )
    };

    window.paint_quad(fill(
        terminal_link_background_bounds(row_origin, x, width, line_height),
        background,
    ));
    paint_terminal_underline_quad(
        row_origin,
        line_height,
        TerminalPaintQuad {
            x: px(hovered_link.col_range.start as f32 * surface.metrics.cell_width_px),
            width,
            color: underline,
        },
        thickness,
        window,
    );
}

fn paint_terminal_underline_quad(
    row_origin: gpui::Point<Pixels>,
    line_height: Pixels,
    underline: TerminalPaintQuad,
    thickness: Pixels,
    window: &mut Window,
) {
    let (x, width) = terminal_link_horizontal_bounds(row_origin.x + underline.x, underline.width);
    if width <= px(0.0) {
        return;
    }

    window.paint_quad(fill(
        Bounds::new(
            point(x, row_origin.y + line_height - thickness),
            size(width, thickness),
        ),
        underline.color,
    ));
}

fn paint_terminal_link_background_quad(
    row_origin: gpui::Point<Pixels>,
    line_height: Pixels,
    highlight: TerminalPaintQuad,
    window: &mut Window,
) {
    let x = row_origin.x + highlight.x;
    let bounds = terminal_link_background_bounds(row_origin, x, highlight.width, line_height);
    if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
        return;
    }

    window.paint_quad(fill(bounds, highlight.color));
}

fn terminal_link_background_bounds(
    row_origin: gpui::Point<Pixels>,
    x: Pixels,
    width: Pixels,
    line_height: Pixels,
) -> Bounds<Pixels> {
    let (x, width) = terminal_link_horizontal_bounds(x, width);

    Bounds::new(point(x, row_origin.y), size(width, line_height))
}

fn terminal_link_horizontal_bounds(x: Pixels, width: Pixels) -> (Pixels, Pixels) {
    let left_inset = px(TERMINAL_LINK_LEFT_INSET_PX);
    let right_inset = px(TERMINAL_LINK_RIGHT_INSET_PX);
    let adjusted_x = x + left_inset;
    let adjusted_width = (width - left_inset - right_inset).max(px(0.0));

    (adjusted_x, adjusted_width)
}

fn selection_span_for_row(
    selection: TerminalSelection,
    row_index: usize,
    visible_cols: usize,
) -> Option<(usize, usize)> {
    let (start, end) = ordered_selection_points(selection);
    if start == end || row_index < start.row || row_index > end.row {
        return None;
    }

    Some(if start.row == end.row {
        (start.col.min(visible_cols), end.col.min(visible_cols))
    } else if row_index == start.row {
        (start.col.min(visible_cols), visible_cols)
    } else if row_index == end.row {
        (0, end.col.min(visible_cols))
    } else {
        (0, visible_cols)
    })
}

fn ordered_selection_points(
    selection: TerminalSelection,
) -> (TerminalSelectionPoint, TerminalSelectionPoint) {
    if (selection.anchor.row, selection.anchor.col) <= (selection.focus.row, selection.focus.col) {
        (selection.anchor, selection.focus)
    } else {
        (selection.focus, selection.anchor)
    }
}

pub(crate) fn terminal_glyph_policy(cell: &TerminalCell) -> TerminalGlyphPolicy {
    if cell.width > 1 {
        return TerminalGlyphPolicy::WideCell;
    }

    let mut chars = cell.text.chars();
    let Some(first) = chars.next() else {
        return TerminalGlyphPolicy::GroupableAscii;
    };

    if first.is_ascii() && !chars.any(|ch| !ch.is_ascii()) && !first.is_ascii_control() {
        return TerminalGlyphPolicy::GroupableAscii;
    }

    let _ = is_terminal_special_glyph(first);
    TerminalGlyphPolicy::PerCellSpecial
}

pub(crate) fn is_terminal_special_glyph(ch: char) -> bool {
    matches!(
        ch as u32,
        0x2500..=0x257f | 0x2580..=0x259f | 0x2800..=0x28ff | 0xe000..=0xf8ff
    )
}

pub(crate) fn resolve_terminal_foreground(style: TerminalCellStyle, theme: &Theme) -> gpui::Hsla {
    let base = style
        .foreground
        .map(terminal_color_to_hsla)
        .unwrap_or(theme.text_primary);

    if !style.faint {
        return base;
    }

    soften_faint_terminal_foreground(base, theme)
}

pub(crate) fn soften_faint_terminal_foreground(base: gpui::Hsla, theme: &Theme) -> gpui::Hsla {
    let subdued = base.blend(theme.bg_void.alpha(0.62)).alpha(0.78);
    let cap = if lightness_distance(theme.text_ghost, theme.bg_void) >= 0.10 {
        theme.text_ghost
    } else {
        theme.text_muted
    };
    let subdued = if lightness_distance(subdued, theme.bg_void) < 0.10 {
        cap
    } else {
        subdued
    };

    if lightness_distance(subdued, theme.bg_void) > lightness_distance(cap, theme.bg_void) {
        subdued.blend(cap.alpha(0.55))
    } else {
        subdued
    }
}

pub(crate) fn lightness_distance(left: gpui::Hsla, right: gpui::Hsla) -> f32 {
    (left.l - right.l).abs()
}

pub(crate) fn terminal_color_to_hsla(color: TerminalColor) -> gpui::Hsla {
    gpui::rgb((u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b)).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;
    use seance_terminal::{TerminalCell, TerminalCellStyle, TerminalColor, TerminalRow};

    fn metrics() -> TerminalMetrics {
        TerminalMetrics {
            cell_width_px: 8.0,
            cell_height_px: 19.0,
            line_height_px: 19.0,
            font_size_px: 13.0,
        }
    }

    #[test]
    fn row_plans_preserve_visible_column_count() {
        let row = TerminalRow {
            cells: vec![
                TerminalCell {
                    text: "a".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                },
                TerminalCell {
                    text: "bc".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                },
                TerminalCell {
                    text: "界".into(),
                    style: TerminalCellStyle::default(),
                    width: 2,
                },
            ],
        };

        let mut metrics = TerminalRendererMetrics::default();
        let segments =
            terminal_fragment_plans(&row, 6, &ThemeId::ObsidianSmoke.theme(), &mut metrics);

        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.cell_count)
                .sum::<usize>(),
            4
        );
    }

    #[test]
    fn special_glyphs_render_per_cell() {
        let box_cell = TerminalCell {
            text: "┌".into(),
            style: TerminalCellStyle::default(),
            width: 1,
        };
        let braille_cell = TerminalCell {
            text: "⣶".into(),
            style: TerminalCellStyle::default(),
            width: 1,
        };
        let private_use_cell = TerminalCell {
            text: "\u{e0b0}".into(),
            style: TerminalCellStyle::default(),
            width: 1,
        };
        let ascii_cell = TerminalCell {
            text: "A".into(),
            style: TerminalCellStyle::default(),
            width: 1,
        };

        assert_eq!(
            terminal_glyph_policy(&box_cell),
            TerminalGlyphPolicy::PerCellSpecial
        );
        assert_eq!(
            terminal_glyph_policy(&braille_cell),
            TerminalGlyphPolicy::PerCellSpecial
        );
        assert_eq!(
            terminal_glyph_policy(&private_use_cell),
            TerminalGlyphPolicy::PerCellSpecial
        );
        assert_eq!(
            terminal_glyph_policy(&ascii_cell),
            TerminalGlyphPolicy::GroupableAscii
        );
    }

    #[test]
    fn clips_wide_cells_at_visible_edge() {
        let row = TerminalRow {
            cells: vec![
                TerminalCell {
                    text: "A".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                },
                TerminalCell {
                    text: "界".into(),
                    style: TerminalCellStyle::default(),
                    width: 2,
                },
            ],
        };

        let mut metrics = TerminalRendererMetrics::default();
        let segments =
            terminal_fragment_plans(&row, 2, &ThemeId::ObsidianSmoke.theme(), &mut metrics);

        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.cell_count)
                .sum::<usize>(),
            1
        );
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].text, "A");
        assert_eq!(segments[0].cell_count, 1);
    }

    #[test]
    fn background_quads_merge_adjacent_cells() {
        let row = TerminalRow {
            cells: vec![
                TerminalCell {
                    text: "A".into(),
                    style: TerminalCellStyle {
                        background: Some(TerminalColor { r: 1, g: 2, b: 3 }),
                        ..TerminalCellStyle::default()
                    },
                    width: 1,
                },
                TerminalCell {
                    text: "B".into(),
                    style: TerminalCellStyle {
                        background: Some(TerminalColor { r: 1, g: 2, b: 3 }),
                        ..TerminalCellStyle::default()
                    },
                    width: 1,
                },
            ],
        };

        let quads = terminal_background_quads(&row, 4, metrics(), &ThemeId::ObsidianSmoke.theme());

        assert_eq!(quads.len(), 1);
        assert_eq!(quads[0].width, px(16.0));
    }

    #[test]
    fn faint_text_is_softened_for_ghost_text_rendering() {
        let theme = ThemeId::Bone.theme();
        let base = gpui::rgb(0x1a1816).into();

        let softened = soften_faint_terminal_foreground(base, &theme);

        assert!(lightness_distance(softened, theme.bg_void) >= 0.10);
        assert!(
            lightness_distance(softened, theme.bg_void)
                <= lightness_distance(theme.text_muted, theme.bg_void)
        );
    }

    #[test]
    fn hyperlink_quads_are_emitted_without_ansi_underline() {
        let row = TerminalRow {
            cells: "https://example.com"
                .chars()
                .map(|ch| TerminalCell {
                    text: ch.to_string(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                })
                .collect(),
        };
        let theme = ThemeId::ObsidianSmoke.theme();
        let link_paint = terminal_link_paint(&row, row.terminal_width(), metrics(), &theme);
        let ansi_underlines =
            terminal_underline_quads(&row, row.terminal_width(), metrics(), &theme);

        assert_eq!(link_paint.highlights.len(), 1);
        assert_eq!(link_paint.underlines.len(), 1);
        assert!(ansi_underlines.is_empty());
    }

    #[test]
    fn hyperlink_quads_stay_separate_from_ansi_underlines() {
        let row = TerminalRow {
            cells: "https://example.com"
                .chars()
                .map(|ch| TerminalCell {
                    text: ch.to_string(),
                    style: TerminalCellStyle {
                        underline: true,
                        ..TerminalCellStyle::default()
                    },
                    width: 1,
                })
                .collect(),
        };
        let theme = ThemeId::ObsidianSmoke.theme();
        let link_paint = terminal_link_paint(&row, row.terminal_width(), metrics(), &theme);
        let ansi_underlines =
            terminal_underline_quads(&row, row.terminal_width(), metrics(), &theme);

        assert_eq!(link_paint.underlines.len(), 1);
        assert_eq!(ansi_underlines.len(), 1);
        assert_ne!(link_paint.underlines[0].color, ansi_underlines[0].color);
    }

    #[test]
    fn hovered_link_uses_stronger_colors_than_rest_state() {
        let theme = ThemeId::ObsidianSmoke.theme();

        assert!(theme.terminal_link_hover_bg.a > theme.terminal_link.a);
        assert!(theme.terminal_link_hover_underline.a > theme.terminal_link_underline.a);
        assert!(theme.terminal_link_modifier_bg.a > theme.terminal_link_hover_bg.a);
        assert!(theme.terminal_link_modifier_underline.a >= theme.terminal_link_hover_underline.a);
    }

    #[test]
    fn link_paint_is_empty_for_non_link_rows() {
        let row = TerminalRow {
            cells: "not a link"
                .chars()
                .map(|ch| TerminalCell {
                    text: ch.to_string(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                })
                .collect(),
        };
        let theme = ThemeId::ObsidianSmoke.theme();
        let link_paint = terminal_link_paint(&row, row.terminal_width(), metrics(), &theme);

        assert!(link_paint.ranges.is_empty());
        assert!(link_paint.highlights.is_empty());
        assert!(link_paint.underlines.is_empty());
    }

    #[test]
    fn right_edge_trim_is_asymmetric() {
        let start_x = px(12.0);
        let width = px(80.0);
        let (trimmed_x, trimmed_width) = terminal_link_horizontal_bounds(start_x, width);

        assert_eq!(trimmed_x, start_x + px(TERMINAL_LINK_LEFT_INSET_PX));
        assert_eq!(
            trimmed_width,
            width - px(TERMINAL_LINK_LEFT_INSET_PX) - px(TERMINAL_LINK_RIGHT_INSET_PX)
        );
        assert!(TERMINAL_LINK_RIGHT_INSET_PX > TERMINAL_LINK_LEFT_INSET_PX);
    }

    #[test]
    fn underline_and_background_share_same_horizontal_bounds() {
        let row_origin = point(px(8.0), px(4.0));
        let line_height = px(20.0);
        let quad_x = row_origin.x + px(16.0);
        let quad_width = px(72.0);

        let background_bounds =
            terminal_link_background_bounds(row_origin, quad_x, quad_width, line_height);
        let (underline_x, underline_width) = terminal_link_horizontal_bounds(quad_x, quad_width);

        assert_eq!(background_bounds.origin.x, underline_x);
        assert_eq!(background_bounds.size.width, underline_width);
    }

    #[test]
    fn narrow_link_does_not_panic_or_render_negative_width() {
        let row_origin = point(px(0.0), px(0.0));
        let line_height = px(20.0);
        let tiny_width = px(2.0);
        let (trimmed_x, trimmed_width) = terminal_link_horizontal_bounds(px(10.0), tiny_width);
        let bounds = terminal_link_background_bounds(row_origin, px(10.0), tiny_width, line_height);

        assert_eq!(trimmed_x, px(10.0) + px(TERMINAL_LINK_LEFT_INSET_PX));
        assert_eq!(trimmed_width, px(0.0));
        assert!(trimmed_width >= px(0.0));
        assert_eq!(bounds.size.width, px(0.0));
    }

    #[test]
    fn width_drift_threshold_is_strictly_greater_than_quarter_px() {
        let plan = TerminalFragmentPlan {
            text: "abcd".into(),
            style: TerminalCellStyle::default(),
            glyph_policy: TerminalGlyphPolicy::GroupableAscii,
            start_col: 0,
            cell_count: 4,
        };

        assert!(!should_cell_align_fragment(&plan, 0.25));
        assert!(should_cell_align_fragment(&plan, 0.251));
    }

    #[test]
    fn width_drift_fallback_only_applies_to_groupable_ascii() {
        let special_plan = TerminalFragmentPlan {
            text: "┌".into(),
            style: TerminalCellStyle::default(),
            glyph_policy: TerminalGlyphPolicy::PerCellSpecial,
            start_col: 3,
            cell_count: 1,
        };

        assert!(!should_cell_align_fragment(&special_plan, 0.5));
    }

    #[test]
    fn cell_aligned_fallback_splits_ascii_fragment_at_fixed_grid_positions() {
        let plan = TerminalFragmentPlan {
            text: "abcd".into(),
            style: TerminalCellStyle::default(),
            glyph_policy: TerminalGlyphPolicy::GroupableAscii,
            start_col: 4,
            cell_count: 4,
        };
        let cells = cell_aligned_ascii_cells(&plan, 8.3);

        assert_eq!(cells.len(), 4);
        assert_eq!(cells[0].0, "a");
        assert_eq!(cells[1].0, "b");
        assert_eq!(cells[2].0, "c");
        assert_eq!(cells[3].0, "d");
        assert!((f32::from(cells[0].1) - 33.2).abs() < 0.001);
        assert!((f32::from(cells[1].1) - 41.5).abs() < 0.001);
        assert!((f32::from(cells[2].1) - 49.8).abs() < 0.001);
        assert!((f32::from(cells[3].1) - 58.1).abs() < 0.001);
    }
}
