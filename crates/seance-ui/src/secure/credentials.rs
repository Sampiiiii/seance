// Credential list + detail rendering for the secure workspace.

use gpui::{Context, Div, FontWeight, MouseButton, div, prelude::*, px};

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
        let credentials: Vec<_> = self
            .cached_credentials
            .iter()
            .filter(|credential| self.credential_matches_query(credential))
            .collect();

        if credentials.is_empty() {
            return crate::ui_components::empty_state(
                "⊙",
                "No credentials yet",
                "Store encrypted passwords for SSH authentication.",
                &t,
            );
        }

        let mut rows = div().flex().flex_col().gap(px(6.0));
        for credential in credentials {
            let credential_scope_key =
                item_scope_key(&credential.vault_id, &credential.credential.id);
            let selected = self.secure.credential_draft.as_ref().is_some_and(|draft| {
                draft.credential_id.as_deref() == Some(credential.credential.id.as_str())
                    && draft.vault_id.as_deref() == Some(credential.vault_id.as_str())
            });
            rows = rows.child(
                self.render_list_row(
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
                &t,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.secure.input_target = SecureInputTarget::CredentialDraft(field);
                    if let Some(draft) = this.secure.credential_draft.as_mut() {
                        draft.selected_field = field;
                    }
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

        // Footer
        let mut left_actions = div().flex().gap(px(8.0));
        if let Some(credential_id) = draft.credential_id.as_ref() {
            let credential_scope_key = draft
                .vault_id
                .as_ref()
                .map(|vault_id| item_scope_key(vault_id, credential_id));
            left_actions = left_actions.child(danger_button("delete", &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if let Some(credential_scope_key) = credential_scope_key.as_ref() {
                        this.attempt_delete_credential(credential_scope_key, cx);
                    }
                }),
            ));
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
