use gpui::{Div, FontWeight, IntoElement, Pixels, SharedString, Size, div, prelude::*, px};
use seance_config::AppConfig;
use seance_core::UpdateState;
use seance_terminal::{TerminalGeometry, TerminalRow};

use crate::{
    TerminalMetrics, TerminalRendererMetrics,
    forms::SettingsSection,
    perf::{PerfOverlayState, UiPerfMode},
    theme::{Theme, ThemeId},
};

pub(crate) const TERMINAL_PANE_PADDING_PX: f32 = 16.0;

pub(crate) fn theme_id_from_config(config: &AppConfig) -> ThemeId {
    ThemeId::from_key(&config.appearance.theme).unwrap_or(ThemeId::ObsidianSmoke)
}

pub(crate) fn frame_budget_color(frame_ms: f32, theme: &Theme) -> gpui::Hsla {
    if frame_ms <= 16.7 {
        theme.accent
    } else if frame_ms <= 25.0 {
        theme.warning
    } else {
        theme.text_secondary
    }
}

pub(crate) fn perf_status_color(ok: bool, theme: &Theme) -> gpui::Hsla {
    if ok { theme.accent } else { theme.warning }
}

pub(crate) fn perf_mode_label(mode: UiPerfMode) -> &'static str {
    match mode {
        UiPerfMode::Off => "off",
        UiPerfMode::Compact => "compact",
        UiPerfMode::Expanded => "expanded",
    }
}

pub(crate) fn update_status_label(state: &UpdateState) -> &'static str {
    match state {
        UpdateState::Idle => "Idle",
        UpdateState::Checking => "Checking for updates…",
        UpdateState::Available(_) => "Update available",
        UpdateState::Downloading => "Downloading update…",
        UpdateState::Installing => "Installing update…",
        UpdateState::ReadyToRelaunch => "Update ready to relaunch",
        UpdateState::UpToDate => "Séance is up to date.",
        UpdateState::Failed(_) => "Update check failed",
    }
}

pub(crate) fn compact_perf_strings(state: &PerfOverlayState) -> Vec<(&'static str, String)> {
    let terminal = state
        .active_session_perf_snapshot
        .as_ref()
        .map(|snapshot| &snapshot.terminal);
    vec![
        ("fps", format!("{:.0}", state.frame_stats.fps_1s)),
        (
            "frame",
            format!(
                "{:.1}/{:.1}/{:.1} ms",
                state.frame_stats.frame_time_last_ms,
                state.frame_stats.frame_time_avg_ms,
                state.frame_stats.frame_time_p95_ms
            ),
        ),
        (
            "snapshot",
            format!(
                "{:.2} ms",
                terminal
                    .map(|metrics| metrics.last_snapshot_duration.as_secs_f32() * 1_000.0)
                    .unwrap_or_default()
            ),
        ),
        (
            "rows",
            terminal
                .map(|metrics| metrics.rendered_row_count.to_string())
                .unwrap_or_else(|| "0".into()),
        ),
        (
            "cells",
            terminal
                .map(|metrics| metrics.rendered_cell_count.to_string())
                .unwrap_or_else(|| "0".into()),
        ),
    ]
}

pub(crate) fn expanded_perf_strings(
    state: &PerfOverlayState,
    active_session_id: u64,
    palette_open: bool,
    renderer: TerminalRendererMetrics,
) -> Vec<(&'static str, String)> {
    let terminal = state
        .active_session_perf_snapshot
        .as_ref()
        .map(|snapshot| &snapshot.terminal);
    vec![
        ("ui refresh", state.ui_refreshes_last_second().to_string()),
        (
            "terminal refresh",
            state.terminal_refreshes_last_second().to_string(),
        ),
        (
            "presented",
            state.frames_presented_last_second().to_string(),
        ),
        (
            "present/ui",
            format!(
                "{}/{}",
                state.frames_presented_last_second(),
                state.ui_refreshes_last_second()
            ),
        ),
        (
            "cadence",
            format!(
                "{:.1}/{:.1}/{:.1} ms",
                state.frame_stats.present_interval_last_ms,
                state.frame_stats.present_interval_avg_ms,
                state.frame_stats.present_interval_p95_ms
            ),
        ),
        (
            "dirty",
            if state.active_session_dirty() {
                "yes".into()
            } else {
                "no".into()
            },
        ),
        ("vt bytes", state.vt_bytes_per_second().to_string()),
        (
            "truncated",
            terminal
                .map(|metrics| metrics.truncated_row_count.to_string())
                .unwrap_or_else(|| "0".into()),
        ),
        ("session", active_session_id.to_string()),
        (
            "palette",
            if palette_open {
                "open".into()
            } else {
                "closed".into()
            },
        ),
        ("visible", state.visible_line_count.to_string()),
        (
            "reason",
            state.frame_stats.redraw_reason.label().to_string(),
        ),
        ("plan rows", renderer.visible_rows.to_string()),
        ("fragments", renderer.fragments.to_string()),
        ("bg quads", renderer.background_quads.to_string()),
        ("shape hits", renderer.shape_hits.to_string()),
        ("shape misses", renderer.shape_misses.to_string()),
    ]
}

pub(crate) fn perf_row(
    label: &'static str,
    value: String,
    value_color: gpui::Hsla,
    theme: &Theme,
) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .child(div().text_color(theme.text_muted).child(label))
        .child(div().text_color(value_color).child(value))
}

pub(crate) fn settings_section_group(label: &'static str, theme: &Theme) -> Div {
    div()
        .pt(px(10.0))
        .pb(px(2.0))
        .flex()
        .items_center()
        .gap(px(8.0))
        .child(
            div()
                .w(px(4.0))
                .h(px(4.0))
                .rounded_full()
                .bg(theme.accent_glow),
        )
        .child(
            div()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_ghost)
                .child(label.to_uppercase()),
        )
        .child(
            div()
                .flex_1()
                .h(px(1.0))
                .bg(theme.glass_border_bright),
        )
}

pub(crate) fn settings_nav_button(section: SettingsSection, active: bool, theme: &Theme) -> Div {
    let base = div()
        .px(px(12.0))
        .py(px(8.0))
        .rounded_r_md()
        .cursor_pointer();

    let styled = if active {
        base.border_l_2()
            .border_color(theme.accent)
            .bg(theme.accent_glow)
            .hover(|style| style.bg(theme.accent_glow))
    } else {
        base.ml(px(2.0))
            .hover(|style| style.bg(theme.glass_hover))
    };

    styled.child(
        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(if active {
                        theme.accent
                    } else {
                        theme.text_ghost
                    })
                    .child(section.glyph()),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if active {
                        theme.text_primary
                    } else {
                        theme.text_secondary
                    })
                    .child(section.title()),
            ),
    )
}

pub(crate) fn settings_choice_chip(
    label: impl Into<SharedString>,
    active: bool,
    theme: &Theme,
) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(12.0))
        .py(px(5.0))
        .rounded_full()
        .border_1()
        .border_color(if active {
            theme.accent
        } else {
            theme.glass_border
        })
        .bg(if active {
            theme.accent_glow
        } else {
            gpui::transparent_black()
        })
        .text_xs()
        .text_color(if active {
            theme.text_primary
        } else {
            theme.text_secondary
        })
        .cursor_pointer()
        .hover(|style| style.bg(theme.glass_hover))
        .child(label.into())
}

pub(crate) fn settings_action_chip(label: impl Into<SharedString>, theme: &Theme) -> Div {
    div()
        .px(px(12.0))
        .py(px(5.0))
        .rounded_full()
        .border_1()
        .border_color(theme.glass_border_bright)
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text_secondary)
        .cursor_pointer()
        .hover(|style| style.bg(theme.glass_hover).text_color(theme.text_primary))
        .child(label.into())
}

pub(crate) fn settings_toggle_card(
    title: &'static str,
    description: &'static str,
    enabled: bool,
    theme: &Theme,
) -> Div {
    let card = div()
        .p_4()
        .rounded_lg()
        .bg(theme.glass_tint)
        .cursor_pointer()
        .hover(|style| style.bg(theme.glass_hover));

    let card = if enabled {
        card.border_l_2()
            .border_color(theme.accent)
    } else {
        card.border_1()
            .border_color(theme.glass_border)
    };

    card.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(description),
                        ),
                )
                .child(
                    // Pill-shaped toggle
                    div()
                        .w(px(40.0))
                        .h(px(22.0))
                        .rounded_full()
                        .bg(if enabled {
                            theme.accent
                        } else {
                            theme.glass_active
                        })
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .w(px(16.0))
                                .h(px(16.0))
                                .rounded_full()
                                .bg(if enabled {
                                    theme.text_primary
                                } else {
                                    theme.text_ghost
                                })
                                .ml(if enabled { px(21.0) } else { px(3.0) }),
                        ),
                ),
        )
}

pub(crate) fn settings_info_card(
    title: &'static str,
    value: String,
    description: &'static str,
    theme: &Theme,
) -> Div {
    div()
        .p_4()
        .rounded_lg()
        .bg(theme.glass_tint)
        .border_1()
        .border_color(theme.glass_border)
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.text_primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.accent)
                                .child(value),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(description),
                ),
        )
}

pub(crate) fn unlock_field_card(
    label: &'static str,
    value: String,
    selected: bool,
    theme: &Theme,
) -> impl IntoElement {
    let mut card = div()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.glass_border)
        .bg(theme.glass_tint)
        .flex()
        .flex_col()
        .gap_1();
    if selected {
        card = card.border_color(theme.accent_glow).bg(theme.glass_active);
    }

    card.child(div().text_xs().text_color(theme.text_muted).child(label))
        .child(
            div()
                .text_sm()
                .text_color(theme.text_primary)
                .child(if value.is_empty() { " ".into() } else { value }),
        )
}

pub(crate) fn editor_field_card(
    label: &'static str,
    value: String,
    selected: bool,
    theme: &Theme,
) -> impl IntoElement {
    let mut card = div()
        .p_3()
        .rounded_lg()
        .border_1()
        .border_color(theme.glass_border)
        .bg(theme.glass_tint)
        .flex()
        .flex_col()
        .gap_1();
    if selected {
        card = card.border_color(theme.accent_glow).bg(theme.glass_active);
    }

    card.child(div().text_xs().text_color(theme.text_muted).child(label))
        .child(
            div()
                .text_sm()
                .text_color(theme.text_primary)
                .child(if value.is_empty() { " ".into() } else { value }),
        )
}

pub(crate) fn masked_value(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        "•".repeat(value.chars().count())
    }
}

pub(crate) fn compute_terminal_geometry(
    viewport_size: Size<Pixels>,
    metrics: TerminalMetrics,
    sidebar_width: f32,
) -> Option<TerminalGeometry> {
    let pane_width_px = (f32::from(viewport_size.width) - sidebar_width).max(0.0);
    let pane_height_px = f32::from(viewport_size.height).max(0.0);
    let usable_width_px = (pane_width_px - (TERMINAL_PANE_PADDING_PX * 2.0)).max(1.0);
    let usable_height_px = (pane_height_px - (TERMINAL_PANE_PADDING_PX * 2.0)).max(1.0);
    let cols = (usable_width_px / metrics.cell_width_px).floor().max(1.0) as u16;
    let rows = (usable_height_px / metrics.cell_height_px).floor().max(1.0) as u16;

    TerminalGeometry::new(
        cols,
        rows,
        usable_width_px.floor() as u16,
        usable_height_px.floor() as u16,
        metrics.cell_width_px.ceil() as u16,
        metrics.line_height_px.ceil() as u16,
    )
    .ok()
}

pub(crate) fn is_tui_artifact(line: &str) -> bool {
    let non_ws: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    if non_ws.is_empty() {
        return false;
    }
    let special = non_ws
        .iter()
        .filter(|c| {
            matches!(
                **c,
                '\u{2500}'..='\u{257F}'
                    | '\u{2580}'..='\u{259F}'
                    | '\u{2800}'..='\u{28FF}'
            )
        })
        .count();
    (special as f64 / non_ws.len() as f64) > 0.5
}

pub(crate) fn session_preview_text(rows: &[TerminalRow]) -> Option<String> {
    rows.iter()
        .rev()
        .map(TerminalRow::plain_text)
        .find(|line| !line.trim().is_empty() && !is_tui_artifact(line))
}

pub(crate) fn sftp_file_glyph(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "rs" | "py" | "js" | "ts" | "c" | "cpp" | "h" | "go" | "rb" | "java" | "swift" | "kt"
        | "zig" | "hs" | "ml" | "ex" | "exs" | "sh" | "bash" | "zsh" | "fish" | "lua" | "pl"
        | "php" => "\u{2022}",
        "toml" | "yaml" | "yml" | "json" | "xml" | "ini" | "cfg" | "conf" | "env" => "\u{2261}",
        "md" | "txt" | "rst" | "org" | "tex" | "log" => "\u{2630}",
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "bmp" | "webp" | "ico" => "\u{25a3}",
        "zip" | "tar" | "gz" | "bz2" | "xz" | "7z" | "rar" => "\u{29c9}",
        "lock" | "key" | "pem" | "crt" | "cert" => "\u{26bf}",
        _ => "\u{25cb}",
    }
}

pub(crate) fn format_file_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let kb = bytes as f64 / 1024.0;
    if kb < 1024.0 {
        return format!("{kb:.1} K");
    }
    let mb = kb / 1024.0;
    if mb < 1024.0 {
        return format!("{mb:.1} M");
    }
    let gb = mb / 1024.0;
    format!("{gb:.1} G")
}

pub(crate) fn format_unix_perms(mode: u32) -> String {
    let mut s = String::with_capacity(9);
    let flags = [
        (0o400, 'r'),
        (0o200, 'w'),
        (0o100, 'x'),
        (0o040, 'r'),
        (0o020, 'w'),
        (0o010, 'x'),
        (0o004, 'r'),
        (0o002, 'w'),
        (0o001, 'x'),
    ];
    for (bit, ch) in flags {
        if mode & bit != 0 {
            s.push(ch);
        } else {
            s.push('-');
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    use gpui::{px, size};
    use seance_terminal::{TerminalCell, TerminalCellStyle, TerminalRow};

    use crate::model::DEFAULT_SIDEBAR_WIDTH;
    use crate::perf::RedrawReason;

    #[test]
    fn compute_geometry_uses_viewport_minus_sidebar_and_padding() {
        let geometry = compute_terminal_geometry(
            size(px(1280.0), px(820.0)),
            TerminalMetrics {
                cell_width_px: 8.0,
                cell_height_px: 19.0,
                line_height_px: 19.0,
                font_size_px: 13.0,
            },
            DEFAULT_SIDEBAR_WIDTH,
        )
        .expect("geometry");

        assert_eq!(geometry.pixel_size.width_px, 988);
        assert_eq!(geometry.pixel_size.height_px, 788);
        assert_eq!(geometry.size.cols, 123);
        assert_eq!(geometry.size.rows, 41);
    }

    #[test]
    fn compute_geometry_clamps_small_windows_to_one_by_one() {
        let geometry = compute_terminal_geometry(
            size(px(10.0), px(10.0)),
            TerminalMetrics {
                cell_width_px: 20.0,
                cell_height_px: 40.0,
                line_height_px: 40.0,
                font_size_px: 13.0,
            },
            DEFAULT_SIDEBAR_WIDTH,
        )
        .expect("geometry");

        assert_eq!(geometry.size.cols, 1);
        assert_eq!(geometry.size.rows, 1);
    }

    #[test]
    fn preview_text_uses_last_non_empty_row() {
        let rows = vec![
            TerminalRow::default(),
            TerminalRow {
                cells: vec![TerminalCell {
                    text: "prompt$".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                }],
            },
        ];

        assert_eq!(session_preview_text(&rows).as_deref(), Some("prompt$"));
    }

    #[test]
    fn tui_artifact_detects_box_drawing() {
        assert!(is_tui_artifact("┌──────────┐"));
        assert!(is_tui_artifact("│          │"));
        assert!(is_tui_artifact("└──────────┘"));
        assert!(is_tui_artifact("╰───────────────"));
    }

    #[test]
    fn tui_artifact_detects_braille() {
        assert!(is_tui_artifact("⣀⣄⣤⣶⣿⣿⣶⣤⣄⣀"));
    }

    #[test]
    fn tui_artifact_detects_block_elements() {
        assert!(is_tui_artifact("▄▄▄▄▄▄▄▄▄▄"));
        assert!(is_tui_artifact("██████████"));
    }

    #[test]
    fn tui_artifact_allows_normal_text() {
        assert!(!is_tui_artifact("prompt$"));
        assert!(!is_tui_artifact("~/code $ ls -la"));
        assert!(!is_tui_artifact("hello world"));
    }

    #[test]
    fn tui_artifact_allows_mixed_below_threshold() {
        assert!(!is_tui_artifact("status │ ok"));
    }

    #[test]
    fn tui_artifact_empty_and_whitespace() {
        assert!(!is_tui_artifact(""));
        assert!(!is_tui_artifact("   "));
    }

    #[test]
    fn preview_text_skips_tui_artifact_rows() {
        let rows = vec![
            TerminalRow {
                cells: vec![TerminalCell {
                    text: "~/code $".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                }],
            },
            TerminalRow {
                cells: vec![TerminalCell {
                    text: "╰──────────────".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                }],
            },
        ];

        assert_eq!(session_preview_text(&rows).as_deref(), Some("~/code $"));
    }

    #[test]
    fn preview_text_returns_none_when_all_rows_are_artifacts() {
        let rows = vec![
            TerminalRow {
                cells: vec![TerminalCell {
                    text: "┌──────┐".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                }],
            },
            TerminalRow {
                cells: vec![TerminalCell {
                    text: "└──────┘".into(),
                    style: TerminalCellStyle::default(),
                    width: 1,
                }],
            },
        ];

        assert_eq!(session_preview_text(&rows), None);
    }

    #[test]
    fn compact_perf_strings_include_primary_metrics() {
        let mut state = PerfOverlayState::new(UiPerfMode::Compact);
        state.frame_stats.fps_1s = 59.0;
        state.frame_stats.frame_time_last_ms = 12.0;

        let rows = compact_perf_strings(&state);
        let labels = rows.into_iter().map(|(label, _)| label).collect::<Vec<_>>();

        assert_eq!(labels, vec!["fps", "frame", "snapshot", "rows", "cells"]);
    }

    #[test]
    fn expanded_perf_strings_include_render_insights() {
        let mut state = PerfOverlayState::new(UiPerfMode::Expanded);
        state.visible_line_count = 32;
        state.pending_redraw_reason = RedrawReason::Palette;
        state.frame_stats.redraw_reason = RedrawReason::Palette;

        let rows = expanded_perf_strings(&state, 7, true, TerminalRendererMetrics::default());
        let labels = rows.into_iter().map(|(label, _)| label).collect::<Vec<_>>();

        assert!(labels.contains(&"ui refresh"));
        assert!(labels.contains(&"terminal refresh"));
        assert!(labels.contains(&"presented"));
        assert!(labels.contains(&"present/ui"));
        assert!(labels.contains(&"cadence"));
        assert!(labels.contains(&"visible"));
        assert!(labels.contains(&"reason"));
        assert!(labels.contains(&"fragments"));
    }

    #[test]
    fn presented_and_ui_refresh_are_comparable() {
        let mut state = PerfOverlayState::new(UiPerfMode::Expanded);
        let now = Instant::now();

        state.mark_ui_refresh_request(now, RedrawReason::UiRefresh);
        state.mark_ui_refresh_request(now + Duration::from_millis(10), RedrawReason::Palette);
        state.finish_render(
            now + Duration::from_millis(16),
            now + Duration::from_millis(20),
        );

        let rows = expanded_perf_strings(&state, 1, false, TerminalRendererMetrics::default());
        let ratio = rows
            .into_iter()
            .find(|(label, _)| *label == "present/ui")
            .map(|(_, value)| value)
            .unwrap();

        assert_eq!(ratio, "1/2");
    }
}
