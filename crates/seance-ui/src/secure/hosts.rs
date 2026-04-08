// Host list + detail rendering for the secure workspace.

use gpui::{Context, Div, FontWeight, MouseButton, div, prelude::*, px};
use seance_vault::{HostAuthRef, PrivateKeyAlgorithm};

use crate::{
    SeanceWorkspace,
    forms::{HostDraftField, PendingAction, SecureInputTarget},
    ui_components::{editor_field_card, primary_button, settings_action_chip, status_badge},
    workspace::item_scope_key,
};

impl SeanceWorkspace {
    pub(crate) fn render_host_list_content(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let mut rows = div().flex().flex_col().gap(px(6.0));
        let hosts: Vec<_> = self
            .saved_hosts
            .iter()
            .filter(|host| self.host_matches_query(host))
            .collect();

        if hosts.is_empty() {
            return crate::ui_components::empty_state(
                "⌂",
                "No hosts yet",
                "Create a host to store SSH connection profiles.",
                &t,
            );
        }

        for host in hosts {
            let host_scope_key = item_scope_key(&host.vault_id, &host.host.id);
            let selected = self.secure.host_draft.as_ref().is_some_and(|draft| {
                draft.host_id.as_deref() == Some(host.host.id.as_str())
                    && draft.vault_id.as_deref() == Some(host.vault_id.as_str())
            });
            rows = rows.child(
                self.render_list_row(
                    &host.host.label,
                    &format!(
                        "{}@{}:{}  [{}]",
                        host.host.username, host.host.hostname, host.host.port, host.vault_name
                    ),
                    selected,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.begin_edit_host(&host_scope_key, cx);
                    }),
                ),
            );
        }
        rows
    }

    pub(crate) fn render_hosts_detail(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let Some(draft) = self.secure.host_draft.as_ref() else {
            return self.render_placeholder_panel(
                "Select a host",
                "Choose a saved host from the list or create a new one.",
            );
        };

        let dirty_badge = if draft.dirty {
            status_badge("unsaved", t.warning, &t)
        } else {
            status_badge("saved", t.success, &t)
        };

        let mut content = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap_4()
            .p_5()
            // Header
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
                                    .child(if draft.host_id.is_some() {
                                        "Edit Saved Host"
                                    } else {
                                        "New Saved Host"
                                    }),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.text_muted)
                                    .child("Build the connection profile and exact auth order."),
                            ),
                    )
                    .child(dirty_badge),
            );

        if let Some(error) = draft.error.as_ref() {
            content = content.child(self.render_banner(error, true));
        }

        let make_field = |field: HostDraftField, value: String, cx: &mut Context<Self>| {
            let selected = self.secure.input_target == SecureInputTarget::HostDraft(field);
            editor_field_card(field.title(), value, selected, &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.secure.input_target = SecureInputTarget::HostDraft(field);
                    if let Some(draft) = this.secure.host_draft.as_mut() {
                        draft.selected_field = field;
                    }
                    cx.notify();
                }),
            )
        };
        let label_field = make_field(HostDraftField::Label, draft.label.clone(), cx);
        let hostname_field = make_field(HostDraftField::Hostname, draft.hostname.clone(), cx);
        let username_field = make_field(HostDraftField::Username, draft.username.clone(), cx);
        let port_field = make_field(HostDraftField::Port, draft.port.clone(), cx);
        let notes_field = make_field(HostDraftField::Notes, draft.notes.clone(), cx);
        let auth_builder = self.render_host_auth_builder(cx);

        content = content
            .child(
                self.render_section_card(
                    "Connection",
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(label_field)
                        .child(hostname_field)
                        .child(
                            // Username + Port side-by-side
                            div()
                                .flex()
                                .gap(px(8.0))
                                .items_start()
                                .child(div().flex_1().child(username_field))
                                .child(div().w(px(100.0)).child(port_field)),
                        ),
                ),
            )
            .child(self.render_section_card("Authentication", auth_builder))
            .child(self.render_host_tunnels_section(cx))
            .child(self.render_section_card("Notes", div().child(notes_field)));

        // Footer action bar
        let can_save = draft.can_save();
        let footer = div()
            .flex()
            .justify_between()
            .items_center()
            .pt_3()
            .border_t_1()
            .border_color(t.glass_border)
            .child(
                div()
                    .text_xs()
                    .text_color(t.text_ghost)
                    .child("Tab cycles fields · Enter saves"),
            )
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(settings_action_chip("discard", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.request_pending_action(PendingAction::OpenHostDraft(None), cx);
                        }),
                    ))
                    .child(primary_button("save host", can_save, &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.save_host_draft(cx);
                        }),
                    )),
            );

        // Scrollable panel wrapper
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
                    .id("hosts-detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(content),
            )
            .child(div().px_5().pb_4().child(footer))
    }

    fn render_host_auth_builder(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let Some(draft) = self.secure.host_draft.as_ref() else {
            return div();
        };
        let draft_vault_id = draft.vault_id.as_deref();
        let available_credentials = self
            .cached_credentials
            .iter()
            .filter(|item| Some(item.vault_id.as_str()) == draft_vault_id)
            .collect::<Vec<_>>();
        let available_keys = self
            .cached_keys
            .iter()
            .filter(|item| Some(item.vault_id.as_str()) == draft_vault_id)
            .collect::<Vec<_>>();

        // Selected auth sequence
        let mut selected = div().flex().flex_col().gap(px(6.0));
        if draft.auth_items.is_empty() {
            selected = selected.child(
                div()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(t.glass_border)
                    .bg(t.glass_tint)
                    .text_sm()
                    .text_color(t.text_ghost)
                    .child("No auth methods — add one below"),
            );
        } else {
            for (index, auth) in draft.auth_items.iter().enumerate() {
                let (label, glyph, passphrase_id) = match auth {
                    HostAuthRef::Password { credential_id } => (
                        available_credentials
                            .iter()
                            .find(|item| item.credential.id == *credential_id)
                            .map(|item| item.credential.label.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        "⊙",
                        None,
                    ),
                    HostAuthRef::PrivateKey {
                        key_id,
                        passphrase_credential_id,
                    } => (
                        available_keys
                            .iter()
                            .find(|item| item.key.id == *key_id)
                            .map(|item| item.key.label.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        "⚿",
                        passphrase_credential_id.clone(),
                    ),
                };
                let is_selected = draft.selected_auth == Some(index);
                let move_up = index > 0;
                let move_down = index + 1 < draft.auth_items.len();

                let mut card = div().p_3().rounded_lg().border_1().cursor_pointer();

                if is_selected {
                    card = card.border_l_2().border_color(t.accent).bg(t.accent_glow);
                } else {
                    card = card
                        .border_color(t.glass_border)
                        .bg(t.glass_tint)
                        .hover(|s| s.bg(t.glass_hover));
                }

                card = card
                    .flex()
                    .flex_col()
                    .gap_2()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if let Some(draft) = this.secure.host_draft.as_mut() {
                                draft.selected_auth = Some(index);
                            }
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .flex()
                                    .gap(px(6.0))
                                    .items_center()
                                    .child(div().text_sm().text_color(t.text_muted).child(glyph))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::MEDIUM)
                                            .text_color(t.text_primary)
                                            .child(label),
                                    ),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(4.0))
                                    .child(settings_action_chip("✕", &t).on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.remove_auth_item(index, cx);
                                        }),
                                    ))
                                    .when(move_up, |row| {
                                        row.child(settings_action_chip("▲", &t).on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.move_auth_item(index, -1, cx);
                                            }),
                                        ))
                                    })
                                    .when(move_down, |row| {
                                        row.child(settings_action_chip("▼", &t).on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.move_auth_item(index, 1, cx);
                                            }),
                                        ))
                                    }),
                            ),
                    );

                // Passphrase selector for private keys
                if matches!(auth, HostAuthRef::PrivateKey { .. }) {
                    let mut row = div()
                        .flex()
                        .flex_wrap()
                        .gap(px(6.0))
                        .items_center()
                        .pt(px(2.0));
                    let passphrase_selection = passphrase_id.clone();
                    row = row.child(
                        div()
                            .text_xs()
                            .text_color(t.text_ghost)
                            .mr(px(2.0))
                            .child("passphrase:"),
                    );
                    row = row.child(
                        settings_action_chip(
                            if passphrase_id.is_none() {
                                "none"
                            } else {
                                "clear"
                            },
                            &t,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                let next = if passphrase_selection.is_none() {
                                    this.secure
                                        .host_draft
                                        .as_ref()
                                        .and_then(|draft| draft.vault_id.as_deref())
                                        .and_then(|vault_id| {
                                            this.cached_credentials
                                                .iter()
                                                .find(|item| item.vault_id == vault_id)
                                                .map(|item| item.credential.id.clone())
                                        })
                                } else {
                                    None
                                };
                                this.set_auth_passphrase_credential(index, next, cx);
                            }),
                        ),
                    );
                    for credential in &available_credentials {
                        let credential_id = credential.credential.id.clone();
                        let active =
                            passphrase_id.as_deref() == Some(credential.credential.id.as_str());
                        row = row.child(
                            div()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded_full()
                                .border_1()
                                .border_color(if active { t.accent } else { t.glass_border })
                                .bg(if active {
                                    t.accent_glow
                                } else {
                                    gpui::transparent_black()
                                })
                                .text_xs()
                                .text_color(if active {
                                    t.text_primary
                                } else {
                                    t.text_secondary
                                })
                                .cursor_pointer()
                                .hover(|s| s.bg(t.glass_hover))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.set_auth_passphrase_credential(
                                            index,
                                            Some(credential_id.clone()),
                                            cx,
                                        );
                                    }),
                                )
                                .child(credential.credential.label.clone()),
                        );
                    }
                    card = card.child(row);
                }

                selected = selected.child(card);
            }
        }

        // Quick-create actions
        let quick_actions = div()
            .flex()
            .flex_wrap()
            .gap(px(6.0))
            .child(settings_action_chip("+ credential", &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.open_quick_create_credential(cx);
                }),
            ))
            .child(settings_action_chip("+ ed25519", &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.generate_ed25519_key_for_secure(cx);
                }),
            ))
            .child(settings_action_chip("+ rsa-4096", &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.generate_rsa_key_for_secure(cx);
                }),
            ));

        // Available methods
        let mut available = div().flex().flex_col().gap(px(6.0));
        for credential in &available_credentials {
            let credential_id = credential.credential.id.clone();
            available = available.child(
                self.render_list_row(
                    &credential.credential.label,
                    credential
                        .credential
                        .username_hint
                        .as_deref()
                        .unwrap_or("password"),
                    false,
                )
                .child(settings_action_chip("add ⊙", &t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.add_auth_password(&credential_id, cx);
                    }),
                )),
            );
        }
        for key in &available_keys {
            let key_id = key.key.id.clone();
            let algo = match key.key.algorithm {
                PrivateKeyAlgorithm::Ed25519 => "ed25519",
                PrivateKeyAlgorithm::Rsa { .. } => "rsa",
            };
            available = available.child(self.render_list_row(&key.key.label, algo, false).child(
                settings_action_chip("add ⚿", &t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.add_auth_key(&key_id, cx);
                    }),
                ),
            ));
        }

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(quick_actions)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text_ghost)
                    .child("AUTH SEQUENCE"),
            )
            .child(selected)
            .child(div().h(px(1.0)).bg(t.glass_border))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text_ghost)
                    .child("AVAILABLE"),
            )
            .child(available)
    }
}
