use gpui::{App, Bounds, Pixels, SharedString, TextRun, Window, fill, font, point, px, size};
use seance_terminal::{TerminalCell, TerminalCellStyle, TerminalColor, TerminalRow};

use crate::{
    CachedShapeLine, HslaKey, PreparedTerminalSurface, ShapeCache, ShapeCacheKey,
    TerminalFragmentPlan, TerminalGlyphPolicy, TerminalMetrics, TerminalPaintFragment,
    TerminalPaintQuad, TerminalPaintRow, TerminalRendererMetrics, ThemeId, theme::Theme,
};

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
    let underlines = terminal_underline_quads(row, visible_cols, metrics, theme);
    let mut fragments = Vec::with_capacity(fragment_plans.len());

    for plan in fragment_plans {
        if plan.text.is_empty() {
            continue;
        }
        let line = shape_terminal_fragment(
            &plan,
            metrics,
            theme_id,
            theme,
            font_family,
            shape_cache,
            window,
            renderer_metrics,
        );
        fragments.push(TerminalPaintFragment {
            x: px(plan.start_col as f32 * metrics.cell_width_px),
            line,
        });
    }

    renderer_metrics.fragments += fragments.len();
    renderer_metrics.background_quads += backgrounds.len() + underlines.len();

    TerminalPaintRow {
        y: px(row_index as f32 * metrics.line_height_px),
        backgrounds,
        underlines,
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

pub(crate) fn shape_terminal_fragment(
    plan: &TerminalFragmentPlan,
    metrics: TerminalMetrics,
    theme_id: ThemeId,
    theme: &Theme,
    font_family: &str,
    shape_cache: &mut ShapeCache,
    window: &mut Window,
    renderer_metrics: &mut TerminalRendererMetrics,
) -> gpui::ShapedLine {
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
        return entry.line.clone();
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
    line
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
    let line_height = px(surface.line_height_px);

    for row in surface.rows.iter() {
        let row_origin = point(bounds.origin.x, bounds.origin.y + row.y);

        for background in &row.backgrounds {
            window.paint_quad(fill(
                Bounds::new(
                    point(row_origin.x + background.x, row_origin.y),
                    size(background.width, line_height),
                ),
                background.color,
            ));
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

        let quads = terminal_background_quads(
            &row,
            4,
            TerminalMetrics {
                cell_width_px: 8.0,
                cell_height_px: 19.0,
                line_height_px: 19.0,
                font_size_px: 13.0,
            },
            &ThemeId::ObsidianSmoke.theme(),
        );

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
}
