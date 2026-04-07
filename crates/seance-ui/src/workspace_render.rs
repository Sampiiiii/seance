// Owns render-side SeanceWorkspace methods: sidebar, terminal pane, update banner, and overlays.

use gpui::{App, Context, Div, FontWeight, MouseButton, Window, canvas, div, prelude::*, px};
use seance_core::UpdateState;
use std::time::Instant;
use tracing::trace;

use crate::{
    SIDEBAR_FONT_MONO, SIDEBAR_MONO_SIZE_PX, SeanceWorkspace,
    forms::{CredentialField, SettingsSection, UnlockMode},
    model::{TerminalMetrics, ToastState},
    palette::{PaletteGroup, build_items},
    perf::{RedrawReason, UiPerfMode},
    surface::PreparedTerminalSurface,
    terminal_paint::paint_terminal_surface,
    theme::ThemeId,
    ui_components::{
        compact_perf_strings, editor_field_card, expanded_perf_strings,
        frame_budget_color, masked_value, perf_mode_label, perf_row, perf_status_color,
        settings_action_chip, unlock_field_card, update_status_label,
    },
};

impl SeanceWorkspace {
    pub(crate) fn show_toast(&mut self, message: impl Into<String>) {
        self.toast = Some(ToastState {
            message: message.into(),
            shown_at: Instant::now(),
        });
    }

    pub(crate) fn render_toast(&self) -> impl IntoElement {
        let t = self.theme();
        let message = self
            .toast
            .as_ref()
            .map(|t| t.message.clone())
            .unwrap_or_default();

        div()
            .absolute()
            .bottom(px(24.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .px(px(16.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .bg(t.glass_strong)
                    .border_1()
                    .border_color(t.glass_border)
                    .shadow_md()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(12.0))
                    .text_color(t.text_secondary)
                    .child(message),
            )
    }

    pub(crate) fn sidebar_row_shell(&self, active: bool) -> Div {
        let t = self.theme();
        let row = div()
            .px(px(12.0))
            .py(px(6.0))
            .cursor_pointer()
            .flex()
            .items_center()
            .gap(px(8.0));

        if active {
            row.border_l_2()
                .border_color(t.sidebar_indicator)
                .bg(t.sidebar_row_active)
                .rounded_r_md()
                .shadow_sm()
        } else {
            row.ml(px(2.0))
                .rounded_r_md()
                .hover(|style| style.bg(t.sidebar_row_hover))
        }
    }

    pub(crate) fn render_sidebar_section_heading(&self, label: &'static str, meta: String) -> Div {
        let t = self.theme();

        div()
            .px(px(14.0))
            .py(px(4.0))
            .flex()
            .items_center()
            .gap(px(6.0))
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.sidebar_section_label)
                    .child(format!("-- {label}")),
            )
            .child(div().flex_1().h(px(1.0)).bg(t.accent_glow))
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(t.sidebar_meta)
                    .child(meta),
            )
    }

    fn render_sidebar_header(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let mode_label = if self.selected_host_id.is_some() {
            "[ssh]"
        } else {
            "[local]"
        };

        div()
            .pt(px(36.0))
            .px(px(14.0))
            .pb(px(10.0))
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.text_primary)
                            .child("séance"),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                            .text_color(t.sidebar_meta)
                            .child(mode_label),
                    ),
            )
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(t.text_ghost)
                    .cursor_pointer()
                    .hover(|style| style.text_color(t.text_muted))
                    .child("^K")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.toggle_palette(cx);
                        }),
                    ),
            )
    }

    fn render_sidebar_footer(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let vault_status = self.backend.vault_status();
        let device_unlock_warning = vault_status.device_unlock_message.clone();

        let vault_label = if vault_status.unlocked {
            "unlocked"
        } else {
            "locked"
        };

        let mut footer = div()
            .px(px(14.0))
            .pb(px(10.0))
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(div().h(px(1.0)).bg(t.sidebar_separator))
            .child({
                let mut theme_row = div().flex().items_center().gap(px(5.0)).flex_wrap();
                let active_theme = self.active_theme;
                for &tid in ThemeId::ALL {
                    let tid_theme = tid.theme();
                    let is_active = tid == active_theme;
                    let accent_color = tid_theme.accent;
                    theme_row = theme_row.child(
                        div()
                            .w(px(10.0))
                            .h(px(10.0))
                            .rounded_full()
                            .bg(accent_color)
                            .cursor_pointer()
                            .when(is_active, |el| {
                                el.border_1().border_color(t.text_secondary).shadow_sm()
                            })
                            .hover(|s| s.border_1().border_color(t.sidebar_edge_bright))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.persist_theme(tid, window, cx);
                                    this.perf_overlay.mark_input(RedrawReason::Input);
                                }),
                            ),
                    );
                }
                theme_row
            })
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .cursor_pointer()
                            .hover(|style| style.text_color(t.text_secondary))
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                    .text_color(t.sidebar_section_label)
                                    .child("vault:"),
                            )
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                    .text_color(if vault_status.unlocked {
                                        t.accent
                                    } else {
                                        t.warning
                                    })
                                    .child(vault_label),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    if this.vault_unlocked() {
                                        this.lock_vault(cx);
                                    } else {
                                        this.unlock_form.reset_for_unlock();
                                        this.unlock_form.message = Some(
                                            "Enter the recovery passphrase to unlock the vault."
                                                .into(),
                                        );
                                        this.perf_overlay.mark_input(RedrawReason::Input);
                                        cx.notify();
                                    }
                                }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                    .text_color(t.text_ghost)
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(t.text_secondary))
                                    .child("⚙ settings")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.open_settings_panel(SettingsSection::General, cx);
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                    .text_color(t.text_ghost)
                                    .cursor_pointer()
                                    .hover(|style| style.text_color(t.text_muted))
                                    .child("^K")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.toggle_palette(cx);
                                        }),
                                    ),
                            ),
                    ),
            );

        if let Some(message) = device_unlock_warning.as_ref().map(|message| {
            if message.contains("re-enroll") {
                "Touch ID needs re-enrollment.".to_string()
            } else {
                "Touch ID unavailable in this build.".to_string()
            }
        }) {
            footer = footer.child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(t.warning)
                    .line_clamp(2)
                    .child(message),
            );
        }

        footer
    }

    pub(crate) fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();

        div()
            .w(px(self.sidebar_width))
            .h_full()
            .flex()
            .flex_col()
            .justify_between()
            .bg(t.sidebar_bg_elevated)
            .child({
                let mut content = div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(16.0))
                    .child(self.render_sidebar_header(cx))
                    .child(self.render_hosts_section(cx))
                    .child(self.render_sessions_section(cx));
                if self.vault_unlocked() {
                    content = content.child(self.render_vault_section(cx));
                }
                content
            })
            .child(self.render_sidebar_footer(cx))
    }

    pub(crate) fn render_terminal_shell(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex_1()
            .h_full()
            .flex()
            .child(self.render_terminal_pane(window, cx))
    }

    fn render_terminal_pane(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let t = self.theme();

        let base = div()
            .flex_1()
            .h_full()
            .bg(t.bg_void)
            .track_focus(&self.focus_handle)
            .on_mouse_down(MouseButton::Left, {
                let fh = self.focus_handle.clone();
                move |_: &gpui::MouseDownEvent, window: &mut Window, _cx: &mut App| {
                    window.focus(&fh);
                }
            })
            .on_key_down(cx.listener(Self::handle_key_down));

        let sessions = self.sessions();
        if sessions.is_empty() || self.active_session().is_none() {
            self.perf_overlay.visible_line_count = 0;
            return base
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_4()
                .child(
                    div()
                        .text_size(px(48.0))
                        .text_color(t.text_ghost)
                        .child("◈"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(t.text_muted)
                        .child("No active sessions"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(t.glass_border)
                                .bg(t.glass_tint)
                                .text_xs()
                                .text_color(t.text_secondary)
                                .child("⌘K"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.text_muted)
                                .child("to open command palette"),
                        ),
                );
        }

        self.sync_terminal_surface(window);
        self.perf_overlay.visible_line_count = self.terminal_surface.metrics.visible_rows;
        let prepared = PreparedTerminalSurface {
            rows: self.terminal_surface.rows.clone(),
            line_height_px: self
                .terminal_metrics
                .unwrap_or(TerminalMetrics {
                    cell_width_px: 8.0,
                    cell_height_px: self.terminal_line_height_px(),
                    line_height_px: self.terminal_line_height_px(),
                    font_size_px: self.terminal_font_size_px(),
                })
                .line_height_px,
        };
        let exit_status = self
            .active_session()
            .and_then(|session| session.snapshot().exit_status);

        let mut content = div().flex_1().flex().flex_col().gap(px(12.0));

        if matches!(
            self.update_state,
            UpdateState::Available(_)
                | UpdateState::Checking
                | UpdateState::Downloading
                | UpdateState::Installing
                | UpdateState::ReadyToRelaunch
                | UpdateState::Failed(_)
        ) {
            content = content.child(self.render_update_banner(cx));
        }

        content = content.child(
            canvas(
                move |_bounds, _window, _cx| prepared,
                move |bounds, prepared, window, cx| {
                    paint_terminal_surface(bounds, prepared, window, cx);
                },
            )
            .size_full(),
        );

        let mut term = base.p_4().child(content);

        if let Some(exit_status) = exit_status {
            term = term.child(
                div()
                    .mt_3()
                    .text_xs()
                    .text_color(t.warning)
                    .child(format!("[process exited: {exit_status}]")),
            );
        }

        trace!(
            visible_line_count = self.perf_overlay.visible_line_count,
            visible_cell_count = self.terminal_surface.metrics.visible_cells,
            fragments = self.terminal_surface.metrics.fragments,
            background_quads = self.terminal_surface.metrics.background_quads,
            special_glyph_cells = self.terminal_surface.metrics.special_glyph_cells,
            wide_cells = self.terminal_surface.metrics.wide_cells,
            palette_open = self.palette_open,
            "rendered terminal pane"
        );

        term
    }

    fn render_update_banner(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let mut actions = div().flex().flex_wrap().gap(px(8.0));

        if matches!(self.update_state, UpdateState::Available(_)) {
            actions = actions.child(settings_action_chip("install update", &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.install_available_update(cx);
                }),
            ));
        }

        if matches!(self.update_state, UpdateState::Failed(_))
            || matches!(self.update_state, UpdateState::Available(_))
        {
            actions = actions.child(settings_action_chip("dismiss", &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.dismiss_update_banner(cx);
                }),
            ));
        }

        let description = match &self.update_state {
            UpdateState::Available(update) => format!(
                "Version {} is available from the stable channel.",
                update.version
            ),
            UpdateState::Failed(error) => error.clone(),
            _ => update_status_label(&self.update_state).to_string(),
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(16.0))
            .p_4()
            .rounded_xl()
            .bg(t.glass_tint)
            .border_1()
            .border_color(t.glass_border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text_primary)
                            .child("Updater"),
                    )
                    .child(div().text_xs().text_color(t.text_muted).child(description)),
            )
            .child(actions)
    }

    pub(crate) fn render_palette_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let session_labels = self.palette_session_labels();
        let remote_ids = self.remote_session_ids();
        let sessions = self.sessions();
        let items = build_items(
            &sessions,
            &session_labels,
            &self.saved_hosts,
            &self.cached_credentials,
            &self.cached_keys,
            self.active_session_id,
            self.active_theme,
            &self.palette_query,
            self.vault_unlocked(),
            &remote_ids,
            &self.update_state,
        );
        trace!(palette_items = items.len(), "rendered palette overlay");
        let selected = self.palette_selected.min(items.len().saturating_sub(1));
        let show_groups = self.palette_query.is_empty();

        let mut item_list = div().flex().flex_col().py_1();

        if items.is_empty() {
            item_list = item_list.child(
                div()
                    .py_3()
                    .flex()
                    .justify_center()
                    .text_sm()
                    .text_color(t.text_muted)
                    .child("No matching commands"),
            );
        }

        let mut prev_group: Option<PaletteGroup> = None;

        for (idx, item) in items.iter().enumerate() {
            if show_groups {
                let cur_group = item.group;
                if prev_group.map_or(true, |pg| pg != cur_group) {
                    let is_first = prev_group.is_none();
                    let mut header = div()
                        .px_4()
                        .pt(px(if is_first { 6.0 } else { 12.0 }))
                        .pb(px(4.0))
                        .flex()
                        .items_center()
                        .gap_2();

                    header = header
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .text_color(t.palette_group_label)
                                .child(cur_group.label()),
                        )
                        .child(div().flex_1().h(px(1.0)).bg(t.palette_group_separator));

                    item_list = item_list.child(header);
                    prev_group = Some(cur_group);
                }
            }

            let is_sel = idx == selected;
            let action = item.action.clone();

            let mut row = div()
                .mx_2()
                .px_2()
                .py(px(7.0))
                .rounded_lg()
                .flex()
                .items_center()
                .gap_3()
                .cursor_pointer();

            row = if is_sel {
                row.bg(t.selection_soft)
                    .child(div().w(px(2.0)).h(px(20.0)).rounded_full().bg(t.accent))
            } else {
                row.hover(|s| s.bg(t.glass_hover)).child(div().w(px(2.0)))
            };

            let label_el = if !item.match_indices.is_empty() {
                let chars: Vec<char> = item.label.chars().collect();
                let mut label_row = div().flex().items_center().text_sm();
                let mut i = 0;
                while i < chars.len() {
                    let is_match = item.match_indices.contains(&i);
                    let start = i;
                    while i < chars.len() && item.match_indices.contains(&i) == is_match {
                        i += 1;
                    }
                    let segment: String = chars[start..i].iter().collect();
                    let color = if is_match {
                        t.accent
                    } else if is_sel {
                        t.text_primary
                    } else {
                        t.text_secondary
                    };
                    label_row = label_row.child(
                        div()
                            .text_color(color)
                            .font_weight(if is_match {
                                FontWeight::BOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .child(segment),
                    );
                }
                label_row
            } else {
                div()
                    .text_sm()
                    .text_color(if is_sel {
                        t.text_primary
                    } else {
                        t.text_secondary
                    })
                    .child(item.label.clone())
            };

            let content = div().flex_1().child(label_el).child(
                div()
                    .text_xs()
                    .text_color(t.text_muted)
                    .child(item.hint.clone()),
            );

            let mut right_section = div().flex().items_center().gap_2();

            if let Some(shortcut) = item.shortcut {
                right_section = right_section.child(
                    div()
                        .px(px(6.0))
                        .py(px(2.0))
                        .rounded_md()
                        .border_1()
                        .border_color(t.glass_border)
                        .bg(t.glass_tint)
                        .text_xs()
                        .text_color(t.text_ghost)
                        .child(shortcut),
                );
            }

            row = row
                .child(
                    div()
                        .w(px(22.0))
                        .flex()
                        .justify_center()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(if is_sel { t.accent } else { t.text_muted })
                        .child(item.glyph),
                )
                .child(content)
                .child(right_section)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, window, cx| {
                        this.execute_palette_action(action.clone(), window, cx);
                    }),
                );

            item_list = item_list.child(row);
        }

        let scrollable_list = div()
            .id("palette-scroll")
            .max_h(px(420.0))
            .overflow_y_scroll()
            .child(item_list);

        let panel = div()
            .w(px(560.0))
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .rounded_xl()
            .shadow_lg()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(t.glass_border)
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.accent)
                            .font_weight(FontWeight::BOLD)
                            .child("/"),
                    )
                    .child(div().flex_1().flex().items_center().child(
                        if self.palette_query.is_empty() {
                            div()
                                .text_sm()
                                .text_color(t.text_muted)
                                .child("Search commands\u{2026}")
                        } else {
                            div()
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(t.text_primary)
                                        .child(self.palette_query.clone()),
                                )
                                .child(div().w(px(2.0)).h(px(16.0)).ml(px(1.0)).bg(t.accent))
                        },
                    ))
                    .child(
                        div()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded_md()
                            .border_1()
                            .border_color(t.glass_border)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child("esc"),
                    ),
            )
            .child(scrollable_list)
            .child(
                div()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(t.glass_border)
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child("↑↓ navigate")
                            .child("↵ select")
                            .child("esc close"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child(format!("{} commands", items.len())),
                    ),
            );

        div()
            .absolute()
            .size_full()
            .bg(t.scrim)
            .flex()
            .flex_col()
            .items_center()
            .pt(px(100.0))
            .child(panel)
    }

    pub(crate) fn render_unlock_overlay(&self) -> impl IntoElement {
        let t = self.theme();
        let create_mode = matches!(self.unlock_form.mode, UnlockMode::Create);
        let title = if create_mode {
            "Create Vault"
        } else {
            "Unlock Vault"
        };

        let passphrase_card = unlock_field_card(
            "Passphrase",
            masked_value(&self.unlock_form.passphrase),
            self.unlock_form.selected_field == 0,
            &t,
        );

        let mut panel = div()
            .w(px(560.0))
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .rounded_xl()
            .shadow_lg()
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text_primary)
                            .child(title),
                    )
                    .child(
                        div().text_sm().text_color(t.text_muted).child(
                            self.unlock_form
                                .message
                                .clone()
                                .unwrap_or_else(|| "Vault status unknown.".into()),
                        ),
                    ),
            )
            .child(passphrase_card);

        if create_mode {
            panel = panel.child(unlock_field_card(
                "Confirm Passphrase",
                masked_value(&self.unlock_form.confirm_passphrase),
                self.unlock_form.selected_field == 1,
                &t,
            ));
        }

        panel = panel.child(
            div()
                .pt_2()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text_ghost)
                        .child("tab move  enter submit"),
                )
                .child(
                    div()
                        .px_3()
                        .py(px(6.0))
                        .rounded_md()
                        .bg(t.accent_glow)
                        .text_xs()
                        .text_color(t.text_primary)
                        .child(if create_mode {
                            "create vault"
                        } else {
                            "unlock vault"
                        }),
                ),
        );

        div()
            .absolute()
            .size_full()
            .bg(t.scrim)
            .flex()
            .items_center()
            .justify_center()
            .child(panel)
    }

    pub(crate) fn render_credential_editor_overlay(&self) -> impl IntoElement {
        let t = self.theme();
        let Some(editor) = self.credential_editor.as_ref() else {
            return div();
        };

        let title = if editor.credential_id.is_some() {
            "Edit Credential"
        } else {
            "Add Credential"
        };

        let fields = [
            (CredentialField::Label, editor.label.clone(), false),
            (
                CredentialField::UsernameHint,
                editor.username_hint.clone(),
                false,
            ),
            (CredentialField::Secret, editor.secret.clone(), true),
        ];

        let mut panel = div()
            .w(px(520.0))
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .rounded_xl()
            .shadow_lg()
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_muted)
                            .child(editor.message.clone().unwrap_or_default()),
                    ),
            );

        for (idx, (field, value, is_secret)) in fields.into_iter().enumerate() {
            let is_selected = idx == editor.selected_field;
            let display_value = if is_secret && !is_selected {
                "\u{2022}".repeat(value.len().min(20))
            } else {
                value
            };
            panel = panel.child(editor_field_card(
                field.title(),
                display_value,
                is_selected,
                &t,
            ));
        }

        panel = panel.child(
            div()
                .pt_2()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text_ghost)
                        .child("tab move  esc cancel  enter on password saves"),
                )
                .child(
                    div()
                        .px_3()
                        .py(px(6.0))
                        .rounded_md()
                        .bg(t.accent_glow)
                        .text_xs()
                        .text_color(t.text_primary)
                        .child("save credential"),
                ),
        );

        div()
            .absolute()
            .size_full()
            .bg(t.scrim)
            .flex()
            .items_center()
            .justify_center()
            .child(panel)
    }

    pub(crate) fn render_perf_overlay(&self) -> impl IntoElement {
        let t = self.theme();
        let stats = self.perf_overlay.frame_stats;
        let session_perf = self.perf_overlay.active_session_perf_snapshot.as_ref();
        let terminal = session_perf.map(|snapshot| &snapshot.terminal);
        let mode_label = perf_mode_label(self.perf_overlay.mode);
        let compact_rows = compact_perf_strings(&self.perf_overlay);
        let expanded_rows = expanded_perf_strings(
            &self.perf_overlay,
            self.active_session_id,
            self.palette_open,
            self.terminal_surface.metrics,
        );

        let mut panel = div()
            .absolute()
            .top(px(16.0))
            .right(px(16.0))
            .w(px(
                if matches!(self.perf_overlay.mode, UiPerfMode::Expanded) {
                    260.0
                } else {
                    220.0
                },
            ))
            .p_3()
            .rounded_lg()
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .font_family("Menlo")
            .text_xs()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_color(t.accent).child("perf"))
                    .child(div().text_color(t.text_muted).child(mode_label)),
            );

        for (label, value) in compact_rows {
            let color = match label {
                "fps" => perf_status_color(stats.fps_1s >= 30.0, &t),
                "frame" => {
                    frame_budget_color(stats.frame_time_p95_ms.max(stats.frame_time_last_ms), &t)
                }
                "snapshot" => perf_status_color(terminal.is_some(), &t),
                _ => t.text_secondary,
            };
            panel = panel.child(perf_row(label, value, color, &t));
        }

        if matches!(self.perf_overlay.mode, UiPerfMode::Expanded) {
            for (label, value) in expanded_rows {
                let color = match label {
                    "present/ui" => {
                        let ui_refreshes = self.perf_overlay.ui_refreshes_last_second();
                        let ok = ui_refreshes == 0
                            || self.perf_overlay.frames_presented_last_second() <= ui_refreshes;
                        perf_status_color(ok, &t)
                    }
                    "dirty" => perf_status_color(self.perf_overlay.active_session_dirty(), &t),
                    "palette" => perf_status_color(self.palette_open, &t),
                    _ => t.text_secondary,
                };
                panel = panel.child(perf_row(label, value, color, &t));
            }
        }

        panel
    }
}
