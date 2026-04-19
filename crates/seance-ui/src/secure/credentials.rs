// Credential list + detail rendering for the secure workspace.

use gpui::{Context, Div, FontWeight, MouseButton, div, prelude::*, px, uniform_list};

use crate::{
    SeanceWorkspace,
    forms::{CredentialDraftField, CredentialDraftOrigin, PendingAction, SecureInputTarget},
    ui_components::{
        danger_button, editor_field_card, masked_value, primary_button, settings_action_chip,
        status_badge,
    },
    workspace::item_scope_key,
};

impl SeanceWorkspace {
    pub(crate) fn render_credential_list_content(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        if self.secure_filtered_credential_indices.is_empty() {
            return crate::ui_components::empty_state(
                "⊙",
                "No credentials yet",
                "Store encrypted passwords for SSH authentication.",
                &t,
            );
        }

        div().relative().size_full().child(
            uniform_list(
                "secure-credential-list",
                self.secure_filtered_credential_indices.len(),
                cx.processor(|this, range: std::ops::Range<usize>, _window, cx| {
                    let mut rows: Vec<Div> =
                        Vec::with_capacity(range.end.saturating_sub(range.start));
                    for visible_index in range {
                        let Some(credential_index) =
                            this.secure_filtered_credential_indices.get(visible_index)
                        else {
                            continue;
                        };
                        let Some(credential) = this.cached_credentials.get(*credential_index)
                        else {
                            continue;
                        };

                        let credential_scope_key =
                            item_scope_key(&credential.vault_id, &credential.credential.id);
                        let selected = this.secure.credential_draft.as_ref().is_some_and(|draft| {
                            draft.credential_id.as_deref()
                                == Some(credential.credential.id.as_str())
                                && draft.vault_id.as_deref() == Some(credential.vault_id.as_str())
                        });
                        rows.push(
                            this.render_list_row(
                                &credential.credential.label,
                                &format!(
                                    "{}  [{}]",
                                    credential
                                        .credential
                                        .username_hint
                                        .as_deref()
                                        .unwrap_or("password"),
                                    credential.vault_name
                                ),
                                selected,
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.request_pending_action(
                                        PendingAction::OpenCredentialDraft(
                                            Some(credential_scope_key.clone()),
                                            CredentialDraftOrigin::Standalone,
                                        ),
                                        cx,
                                    );
                                }),
                            ),
                        );
                    }
                    rows
                }),
            )
            .size_full()
            .track_scroll(self.secure_credential_list_scroll_handle.clone()),
        )
    }

    pub(crate) fn render_credentials_detail(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let Some(draft) = self.secure.credential_draft.as_ref() else {
            return self.render_placeholder_panel(
                "Select a credential",
                "Choose a credential or create a new one.",
            );
        };

        let field = |field: CredentialDraftField, value: String, masked: bool| {
            let selected = self.secure.input_target == SecureInputTarget::CredentialDraft(field);
            editor_field_card(
                field.title(),
                if masked && !draft.reveal_secret {
                    masked_value(&value)
                } else {
                    value
                },
                selected,
                selected.then_some(&self.secure_text_input),
                &t,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.focus_secure_input_target(SecureInputTarget::CredentialDraft(field));
                    cx.notify();
                }),
            )
        };

        let dirty_badge = if draft.dirty {
            status_badge("unsaved", t.warning, &t)
        } else if draft.credential_id.is_some() {
            status_badge("saved", t.success, &t)
        } else {
            status_badge("new", t.accent, &t)
        };

        let mut content = div()
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
                                    .child(if draft.credential_id.is_some() {
                                        "Edit Credential"
                                    } else {
                                        "New Credential"
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.text_muted)
                                    .child("Passwords are stored encrypted in the vault."),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .items_center()
                            .child(
                                settings_action_chip(
                                    if draft.reveal_secret {
                                        "hide ⊙"
                                    } else {
                                        "show ⊙"
                                    },
                                    &t,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        if let Some(draft) = this.secure.credential_draft.as_mut() {
                                            draft.reveal_secret = !draft.reveal_secret;
                                        }
                                        cx.notify();
                                    }),
                                ),
                            )
                            .child(dirty_badge),
                    ),
            )
            .child(field(
                CredentialDraftField::Label,
                draft.label.clone(),
                false,
            ))
            .child(field(
                CredentialDraftField::UsernameHint,
                draft.username_hint.clone(),
                false,
            ))
            .child(field(
                CredentialDraftField::Secret,
                draft.secret.clone(),
                true,
            ));

        if let Some(error) = draft.error.as_ref() {
            content = content.child(self.render_banner(error, true));
        }

        let host_refs = draft
            .vault_id
            .as_deref()
            .zip(draft.credential_id.as_deref())
            .map(|(vault_id, credential_id)| {
                self.host_references_for_credential(vault_id, credential_id)
            })
            .unwrap_or_default();

        if draft.credential_id.is_some() {
            content = content.child(self.render_section_card(
                "USED BY HOSTS",
                if host_refs.is_empty() {
                    div()
                        .text_sm()
                        .text_color(t.text_ghost)
                        .child("Not referenced by any hosts.")
                } else {
                    let mut list = div().flex().flex_col().gap(px(4.0));
                    for href in host_refs {
                        let host_scope_key = item_scope_key(&href.vault_id, &href.host_id);
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
                                        .cursor_pointer()
                                        .hover(|s| s.text_color(t.text_primary))
                                        .child(format!("{} [{}]", href.host_label, href.vault_name))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.begin_edit_host(&host_scope_key, cx);
                                            }),
                                        ),
                                ),
                        );
                    }
                    list
                },
            ));
        }

        // Footer
        let mut left_actions = div().flex().gap(px(8.0));
        if let Some(credential_id) = draft.credential_id.as_ref() {
            let credential_scope_key = draft
                .vault_id
                .as_ref()
                .map(|vault_id| item_scope_key(vault_id, credential_id));
            let vault_id = draft.vault_id.clone().unwrap_or_default();
            let vault_id_for_passphrase = vault_id.clone();
            let credential_id = credential_id.clone();
            let credential_id_for_passphrase = credential_id.clone();
            left_actions = left_actions.child(danger_button("delete", &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if let Some(credential_scope_key) = credential_scope_key.as_ref() {
                        this.attempt_delete_credential(credential_scope_key, cx);
                    }
                }),
            ));
            left_actions = left_actions
                .child(
                    settings_action_chip("attach as password", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.attach_credential_to_host_password(&vault_id, &credential_id, cx);
                        }),
                    ),
                )
                .child(
                    settings_action_chip("attach as key passphrase", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.attach_credential_to_host_passphrase(
                                &vault_id_for_passphrase,
                                &credential_id_for_passphrase,
                                cx,
                            );
                        }),
                    ),
                );
        }

        let can_save = draft.can_save();
        let footer = div()
            .flex()
            .justify_between()
            .items_center()
            .pt_3()
            .border_t_1()
            .border_color(t.glass_border)
            .child(left_actions)
            .child(
                primary_button("save credential", can_save, &t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.save_credential_draft(cx);
                    }),
                ),
            );

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
                    .id("credentials-detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
            .child(div().px_5().pb_4().child(footer))
    }
}
