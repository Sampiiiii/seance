// SSH key list + detail rendering for the secure workspace.

use gpui::{Context, Div, FontWeight, MouseButton, div, prelude::*, px};

use seance_vault::{PrivateKeyAlgorithm, PrivateKeySource};

use crate::{
    SeanceWorkspace,
    forms::SecureInputTarget,
    ui_components::{danger_button, empty_state, status_badge},
    workspace::{item_scope_key, split_scope_key},
};

impl SeanceWorkspace {
    pub(crate) fn render_key_list_content(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let keys: Vec<_> = self
            .cached_keys
            .iter()
            .filter(|key| self.key_matches_query(key))
            .collect();

        if keys.is_empty() {
            return empty_state(
                "⚿",
                "No SSH keys yet",
                "Generate or import SSH keys for authentication.",
                &t,
            );
        }

        let mut rows = div().flex().flex_col().gap(px(6.0));
        for key in keys {
            let key_scope_key = item_scope_key(&key.vault_id, &key.key.id);
            let selected = self.secure.selected_key_id.as_deref() == Some(key_scope_key.as_str());
            let algo_label = match key.key.algorithm {
                PrivateKeyAlgorithm::Ed25519 => "ed25519",
                PrivateKeyAlgorithm::Rsa { .. } => "rsa",
            };
            rows = rows.child(
                self.render_list_row(
                    &key.key.label,
                    &format!("{}  [{}]", algo_label, key.vault_name),
                    selected,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.secure.selected_key_id = Some(key_scope_key.clone());
                        this.focus_secure_input_target(SecureInputTarget::KeySearch);
                        cx.notify();
                    }),
                ),
            );
        }
        rows
    }

    pub(crate) fn render_keys_detail(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let Some(selected_key_scope) = self.secure.selected_key_id.as_deref() else {
            return self.render_placeholder_panel(
                "Select a key",
                "Choose an SSH key to view its details.",
            );
        };

        let Some((vault_id, key_id)) = split_scope_key(selected_key_scope) else {
            return self.render_placeholder_panel("Invalid key", "Key reference is corrupted.");
        };

        let Some(key_entry) = self
            .cached_keys
            .iter()
            .find(|k| k.key.id == key_id && k.vault_id == vault_id)
        else {
            return self
                .render_placeholder_panel("Key not found", "The selected key no longer exists.");
        };

        let key = &key_entry.key;
        let vault_name = &key_entry.vault_name;

        let algo_label = match key.algorithm {
            PrivateKeyAlgorithm::Ed25519 => "Ed25519",
            PrivateKeyAlgorithm::Rsa { bits } => {
                // Use a static str for common sizes, format for unusual ones.
                if bits == 4096 { "RSA-4096" } else { "RSA" }
            }
        };

        let host_refs = self.host_references_for_key(vault_id, key_id);

        let content = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(3.0))
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(t.text_primary)
                                    .child(key.label.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.text_muted)
                                    .child(format!("Vault: {}", vault_name)),
                            ),
                    )
                    .child(status_badge(algo_label, t.accent, &t)),
            )
            // Key properties
            .child(
                self.render_section_card(
                    "KEY DETAILS",
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(10.0))
                        .child(self.key_detail_row("Algorithm", algo_label, &t))
                        .child(self.key_detail_row(
                            "Source",
                            match key.source {
                                PrivateKeySource::Generated => "Generated locally",
                                PrivateKeySource::Imported => "Imported",
                            },
                            &t,
                        ))
                        .child(self.key_detail_row(
                            "Encrypted at rest",
                            if key.encrypted_at_rest { "Yes" } else { "No" },
                            &t,
                        )),
                ),
            )
            // Used by hosts
            .child(self.render_section_card(
                "USED BY HOSTS",
                if host_refs.is_empty() {
                    div()
                        .text_sm()
                        .text_color(t.text_ghost)
                        .child("Not referenced by any hosts.")
                } else {
                    let mut list = div().flex().flex_col().gap(px(4.0));
                    for href in &host_refs {
                        list = list.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .child(div().text_xs().text_color(t.accent).child("→"))
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(t.text_secondary)
                                        .child(href.label.clone()),
                                ),
                        );
                    }
                    list
                },
            ));

        // Footer
        let key_scope = item_scope_key(vault_id, key_id);
        let footer = div()
            .flex()
            .justify_end()
            .items_center()
            .pt_3()
            .border_t_1()
            .border_color(t.glass_border)
            .child(danger_button("delete key", &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.attempt_delete_private_key(&key_scope, cx);
                }),
            ));

        div()
            .flex_1()
            .h_full()
            .rounded_xl()
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .flex()
            .flex_col()
            .child(
                div()
                    .id("keys-detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
            .child(div().px_5().pb_4().child(footer))
    }

    fn key_detail_row(&self, label: &str, value: &str, t: &crate::theme::Theme) -> Div {
        div()
            .flex()
            .justify_between()
            .items_center()
            .child(
                div()
                    .text_sm()
                    .text_color(t.text_muted)
                    .child(label.to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(t.text_secondary)
                    .child(value.to_string()),
            )
    }
}
