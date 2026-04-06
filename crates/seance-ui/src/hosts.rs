// Owns saved-host CRUD, auth selection, and host-specific UI rendering.

use gpui::{Context, Div, FontWeight, MouseButton, div, prelude::*, px};
use seance_vault::{HostAuthRef, HostSummary, PrivateKeyAlgorithm, VaultHostProfile};

use crate::{
    SIDEBAR_FONT_MONO, SIDEBAR_MONO_SIZE_PX, SeanceWorkspace,
    forms::{HostEditorState, HostField},
    perf::RedrawReason,
    refresh_app_menus,
    ui_components::editor_field_card,
};

impl SeanceWorkspace {
    pub(crate) fn refresh_saved_hosts(&mut self) {
        self.saved_hosts = if self.vault_unlocked() {
            self.backend.list_hosts().unwrap_or_default()
        } else {
            Vec::new()
        };

        if self
            .selected_host_id
            .as_ref()
            .is_some_and(|id| !self.saved_hosts.iter().any(|host| &host.id == id))
        {
            self.selected_host_id = self.saved_hosts.first().map(|host| host.id.clone());
        }
    }

    pub(crate) fn begin_add_host(&mut self, cx: &mut Context<Self>) {
        if !self.vault_unlocked() {
            self.unlock_form.reset_for_unlock();
            self.unlock_form.message = Some("Unlock the vault before adding a saved host.".into());
        } else {
            self.refresh_vault_cache();
            self.host_editor = Some(HostEditorState::blank());
        }
        self.palette_open = false;
        self.perf_overlay.mark_input(RedrawReason::Palette);
        cx.notify();
    }

    pub(crate) fn begin_edit_host(&mut self, host_id: &str, cx: &mut Context<Self>) {
        self.refresh_vault_cache();
        match self.backend.load_host(host_id) {
            Ok(Some(host)) => {
                self.host_editor = Some(HostEditorState::from_host(host));
                self.selected_host_id = Some(host_id.into());
            }
            Ok(None) => {
                self.status_message = Some("Saved host not found.".into());
                self.refresh_saved_hosts();
            }
            Err(err) => {
                self.status_message = Some(err.to_string());
            }
        }
        self.palette_open = false;
        self.perf_overlay.mark_input(RedrawReason::Palette);
        cx.notify();
    }

    pub(crate) fn delete_saved_host(&mut self, host_id: &str, cx: &mut Context<Self>) {
        match self.backend.delete_host(host_id) {
            Ok(true) => {
                self.status_message = Some("Saved host tombstoned for future sync.".into());
                self.refresh_saved_hosts();
            }
            Ok(false) => {
                self.status_message = Some("Saved host already removed.".into());
            }
            Err(err) => {
                self.status_message = Some(err.to_string());
            }
        }
        self.palette_open = false;
        self.perf_overlay.mark_input(RedrawReason::Palette);
        refresh_app_menus(cx);
        cx.notify();
    }

    pub(crate) fn save_host_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.host_editor.as_ref() else {
            return;
        };
        let port = editor.port.trim().parse::<u16>().unwrap_or(22);
        let draft = VaultHostProfile {
            id: editor.host_id.clone().unwrap_or_default(),
            label: editor.label.trim().into(),
            hostname: editor.hostname.trim().into(),
            username: editor.username.trim().into(),
            port,
            notes: (!editor.notes.trim().is_empty()).then(|| editor.notes.trim().to_string()),
            auth_order: editor.auth_items.clone(),
        };

        match self.backend.save_host(draft) {
            Ok(summary) => {
                self.status_message = Some(format!(
                    "Saved host '{}' encrypted into the vault.",
                    summary.label
                ));
                self.host_editor = None;
                self.refresh_saved_hosts();
                self.selected_host_id = Some(summary.id);
            }
            Err(err) => {
                if let Some(editor) = self.host_editor.as_mut() {
                    editor.message = Some(err.to_string());
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        refresh_app_menus(cx);
        cx.notify();
    }

    pub(crate) fn toggle_host_auth_at_cursor(&mut self) {
        let Some(editor) = self.host_editor.as_mut() else {
            return;
        };
        let cred_count = self.cached_credentials.len();
        let cursor = editor.auth_cursor;

        if cursor < cred_count {
            let cred = &self.cached_credentials[cursor];
            let auth_ref = HostAuthRef::Password {
                credential_id: cred.id.clone(),
            };
            if let Some(pos) = editor.auth_items.iter().position(|auth| *auth == auth_ref) {
                editor.auth_items.remove(pos);
            } else {
                editor.auth_items.push(auth_ref);
            }
        } else {
            let key_idx = cursor - cred_count;
            if key_idx < self.cached_keys.len() {
                let key = &self.cached_keys[key_idx];
                let matches_key = |auth: &HostAuthRef| {
                    matches!(auth, HostAuthRef::PrivateKey { key_id, .. } if *key_id == key.id)
                };
                if let Some(pos) = editor.auth_items.iter().position(matches_key) {
                    editor.auth_items.remove(pos);
                } else {
                    editor.auth_items.push(HostAuthRef::PrivateKey {
                        key_id: key.id.clone(),
                        passphrase_credential_id: None,
                    });
                }
            }
        }
    }

    fn render_host_row(&self, host: &HostSummary, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let selected = self
            .selected_host_id
            .as_ref()
            .is_some_and(|id| id == &host.id);
        let is_connecting = self
            .connecting_host_id
            .as_ref()
            .is_some_and(|id| id == &host.id);
        let host_id = host.id.clone();
        let edit_id = host.id.clone();
        let delete_id = host.id.clone();
        let label = host.label.clone();
        let target = format!("{}@{}:{}", host.username, host.hostname, host.port);

        let mut row = self
            .sidebar_row_shell(selected || is_connecting)
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(if selected || is_connecting {
                        theme.accent
                    } else {
                        theme.text_ghost
                    })
                    .child(if is_connecting {
                        "\u{2022}"
                    } else if selected {
                        ">"
                    } else {
                        " "
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(1.0))
                    .overflow_hidden()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if selected || is_connecting {
                                        theme.text_primary
                                    } else {
                                        theme.text_secondary
                                    })
                                    .line_clamp(1)
                                    .child(label),
                            )
                            .child(if is_connecting {
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(9.0))
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(theme.accent_glow)
                                    .text_color(theme.accent)
                                    .child("connecting\u{2026}")
                            } else {
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.0))
                                    .child(
                                        div()
                                            .font_family(SIDEBAR_FONT_MONO)
                                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                            .text_color(theme.text_ghost)
                                            .cursor_pointer()
                                            .hover(|style| style.text_color(theme.text_secondary))
                                            .child("edit")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.begin_edit_host(&edit_id, cx);
                                                }),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .font_family(SIDEBAR_FONT_MONO)
                                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                            .text_color(theme.text_ghost)
                                            .cursor_pointer()
                                            .hover(|style| style.text_color(theme.warning))
                                            .child("del")
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    this.delete_saved_host(&delete_id, cx);
                                                }),
                                            ),
                                    )
                            }),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                            .text_color(theme.sidebar_meta)
                            .line_clamp(1)
                            .child(target),
                    ),
            );

        if !is_connecting {
            row = row.on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, window, cx| {
                    this.selected_host_id = Some(host_id.clone());
                    this.connect_saved_host(&host_id, window, cx);
                }),
            );
        }

        row
    }

    pub(crate) fn render_hosts_section(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let unlocked = self.vault_unlocked();
        let meta = if unlocked {
            self.saved_hosts.len().to_string()
        } else {
            "locked".into()
        };

        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(self.render_sidebar_section_heading("hosts", meta));

        if unlocked {
            if self.saved_hosts.is_empty() {
                section = section.child(
                    div()
                        .px(px(14.0))
                        .font_family(SIDEBAR_FONT_MONO)
                        .text_size(px(SIDEBAR_MONO_SIZE_PX))
                        .text_color(theme.sidebar_meta)
                        .child("no saved hosts"),
                );
            } else {
                let mut rows = div().flex().flex_col();
                for host in &self.saved_hosts {
                    rows = rows.child(self.render_host_row(host, cx));
                }
                section = section.child(rows);
            }

            section = section.child(
                div().px(px(14.0)).pt(px(2.0)).child(
                    div()
                        .font_family(SIDEBAR_FONT_MONO)
                        .text_size(px(SIDEBAR_MONO_SIZE_PX))
                        .text_color(theme.text_ghost)
                        .cursor_pointer()
                        .hover(|style| style.text_color(theme.text_secondary))
                        .child("+ add host")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.begin_add_host(cx);
                            }),
                        ),
                ),
            );
        } else {
            section = section.child(
                div()
                    .px(px(14.0))
                    .py(px(6.0))
                    .cursor_pointer()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(theme.text_muted)
                    .hover(|style| style.text_color(theme.text_secondary))
                    .child("vault locked -- unlock to view")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.unlock_form.reset_for_unlock();
                            this.unlock_form.message =
                                Some("Enter the recovery passphrase to unlock the vault.".into());
                            this.perf_overlay.mark_input(RedrawReason::Input);
                            cx.notify();
                        }),
                    ),
            );
        }

        section
    }

    pub(crate) fn render_host_editor_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme();
        let Some(editor) = self.host_editor.as_ref() else {
            return div();
        };

        let title = if editor.host_id.is_some() {
            "Edit Saved Host"
        } else {
            "Add Saved Host"
        };

        let text_fields: [(HostField, String); 5] = [
            (HostField::Label, editor.label.clone()),
            (HostField::Hostname, editor.hostname.clone()),
            (HostField::Username, editor.username.clone()),
            (HostField::Port, editor.port.clone()),
            (HostField::Notes, editor.notes.clone()),
        ];

        let mut panel = div()
            .w(px(620.0))
            .max_h(px(680.0))
            .overflow_hidden()
            .bg(theme.glass_strong)
            .border_1()
            .border_color(theme.glass_border_bright)
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
                            .text_color(theme.text_primary)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_muted)
                            .child(editor.message.clone().unwrap_or_default()),
                    ),
            );

        for (idx, (field, value)) in text_fields.into_iter().enumerate() {
            panel = panel.child(editor_field_card(
                field.title(),
                value,
                idx == editor.selected_field,
                &theme,
            ));
        }

        panel = panel.child(self.render_host_auth_picker());

        panel = panel.child(
            div()
                .pt_2()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_ghost)
                        .child("tab move  esc cancel  enter/space toggle auth  \u{2318}S save"),
                )
                .child(
                    div()
                        .px_3()
                        .py(px(6.0))
                        .rounded_md()
                        .bg(theme.accent_glow)
                        .text_xs()
                        .text_color(theme.text_primary)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.accent))
                        .child("save encrypted host")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.save_host_editor(cx);
                            }),
                        ),
                ),
        );

        div()
            .absolute()
            .size_full()
            .bg(theme.scrim)
            .flex()
            .items_center()
            .justify_center()
            .child(panel)
    }

    fn render_host_auth_picker(&self) -> Div {
        let theme = self.theme();
        let Some(editor) = self.host_editor.as_ref() else {
            return div();
        };
        let is_auth_field = editor.field() == HostField::Auth;

        let mut section = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(if is_auth_field {
                theme.accent
            } else {
                theme.glass_border
            })
            .bg(theme.glass_tint);

        section = section.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::BOLD)
                        .text_color(if is_auth_field {
                            theme.accent
                        } else {
                            theme.text_muted
                        })
                        .child("Authentication"),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_ghost)
                        .child(if is_auth_field {
                            "enter/space to toggle"
                        } else {
                            "tab to this section"
                        }),
                ),
        );

        if !editor.auth_items.is_empty() {
            let mut selected_list = div().flex().flex_col().gap(px(2.0));
            for (index, auth) in editor.auth_items.iter().enumerate() {
                let label = match auth {
                    HostAuthRef::Password { credential_id } => {
                        let name = self
                            .cached_credentials
                            .iter()
                            .find(|credential| credential.id == *credential_id)
                            .map(|credential| credential.label.as_str())
                            .unwrap_or("unknown");
                        format!("{}. password: {}", index + 1, name)
                    }
                    HostAuthRef::PrivateKey { key_id, .. } => {
                        let name = self
                            .cached_keys
                            .iter()
                            .find(|key| key.id == *key_id)
                            .map(|key| key.label.as_str())
                            .unwrap_or("unknown");
                        format!("{}. key: {}", index + 1, name)
                    }
                };
                selected_list = selected_list.child(
                    div()
                        .text_xs()
                        .font_family(SIDEBAR_FONT_MONO)
                        .text_color(theme.accent)
                        .child(label),
                );
            }
            section = section.child(selected_list);
            section = section.child(div().h(px(1.0)).bg(theme.glass_border));
        }

        let mut all_items: Vec<(String, String, bool)> = Vec::new();

        for credential in &self.cached_credentials {
            let is_selected = editor.auth_items.iter().any(|auth| {
                matches!(auth, HostAuthRef::Password { credential_id } if *credential_id == credential.id)
            });
            let hint = credential.username_hint.as_deref().unwrap_or("");
            let label = if hint.is_empty() {
                format!("password: {}", credential.label)
            } else {
                format!("password: {} ({})", credential.label, hint)
            };
            all_items.push((format!("cred:{}", credential.id), label, is_selected));
        }

        for key in &self.cached_keys {
            let is_selected = editor.auth_items.iter().any(|auth| {
                matches!(auth, HostAuthRef::PrivateKey { key_id, .. } if *key_id == key.id)
            });
            let algo = match &key.algorithm {
                PrivateKeyAlgorithm::Ed25519 => "ed25519",
                PrivateKeyAlgorithm::Rsa { .. } => "rsa",
            };
            let label = format!("key: {} [{}]", key.label, algo);
            all_items.push((format!("key:{}", key.id), label, is_selected));
        }

        if all_items.is_empty() {
            section = section.child(
                div()
                    .py_2()
                    .text_xs()
                    .text_color(theme.text_ghost)
                    .child("No credentials or keys in vault. Add some first."),
            );
        } else {
            let mut rows = div().flex().flex_col();
            for (index, (_item_id, label, selected)) in all_items.iter().enumerate() {
                let is_cursor = is_auth_field && index == editor.auth_cursor;
                let glyph = if *selected { "\u{25c9}" } else { "\u{25cb}" };
                rows = rows.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .bg(if is_cursor {
                            theme.accent_glow
                        } else {
                            gpui::transparent_black()
                        })
                        .child(
                            div()
                                .text_xs()
                                .text_color(if *selected {
                                    theme.accent
                                } else {
                                    theme.text_ghost
                                })
                                .child(glyph),
                        )
                        .child(
                            div()
                                .text_xs()
                                .font_family(SIDEBAR_FONT_MONO)
                                .text_color(if *selected {
                                    theme.text_primary
                                } else {
                                    theme.text_secondary
                                })
                                .child(label.clone()),
                        ),
                );
            }
            section = section.child(rows);
        }

        section
    }
}