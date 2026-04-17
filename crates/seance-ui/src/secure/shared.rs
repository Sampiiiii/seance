// Shared render helpers for the secure workspace: list rows, section cards, banners, placeholders.

use gpui::{Div, FontWeight, div, prelude::*, px};

use crate::{
    SeanceWorkspace,
    ui_components::{editor_field_card, empty_state},
};

impl SeanceWorkspace {
    pub(crate) fn render_search_card(
        &self,
        label: &'static str,
        value: &str,
        selected: bool,
    ) -> Div {
        let t = self.theme();
        editor_field_card(
            label,
            value.to_string(),
            selected,
            selected.then_some(&self.secure_text_input),
            &t,
        )
    }

    pub(crate) fn render_list_row(&self, title: &str, subtitle: &str, selected: bool) -> Div {
        let t = self.theme();
        let mut row = div().p_3().rounded_lg().border_1().cursor_pointer();

        if selected {
            row = row.border_l_2().border_color(t.accent).bg(t.accent_glow);
        } else {
            row = row
                .border_color(t.glass_border)
                .bg(t.glass_tint)
                .hover(|s| s.bg(t.glass_hover));
        }

        row.flex().items_center().justify_between().gap_3().child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .overflow_hidden()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(t.text_primary)
                        .line_clamp(1)
                        .child(title.to_string()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text_muted)
                        .line_clamp(1)
                        .child(subtitle.to_string()),
                ),
        )
    }

    pub(crate) fn render_section_card(
        &self,
        title: &'static str,
        content: impl gpui::IntoElement,
    ) -> Div {
        let t = self.theme();
        div()
            .p_4()
            .rounded_xl()
            .bg(t.glass_tint)
            .border_1()
            .border_color(t.glass_border)
            .border_t_2()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text_ghost)
                    .child(title),
            )
            .child(content)
    }

    pub(crate) fn render_banner(&self, text: &str, warning: bool) -> Div {
        let t = self.theme();
        let color = if warning { t.danger } else { t.accent };
        div()
            .px_4()
            .py(px(10.0))
            .rounded_lg()
            .border_1()
            .border_color(color)
            .bg(t.glass_tint)
            .text_sm()
            .text_color(color)
            .child(text.to_string())
    }

    pub(crate) fn render_placeholder_panel(&self, title: &str, subtitle: &str) -> Div {
        let t = self.theme();
        div()
            .flex_1()
            .h_full()
            .rounded_xl()
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .child(empty_state("◇", title, subtitle, &t))
    }
}
