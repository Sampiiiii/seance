// Owns vault status helpers, credential/key CRUD, and vault-specific UI rendering.

use gpui::{Context, Div, FontWeight, MouseButton, div, prelude::*, px};
use seance_vault::{
    CredentialSummary, KeySummary, PrivateKeyAlgorithm, PrivateKeySource, VaultPasswordCredential,
};

use crate::{
    SIDEBAR_FONT_MONO, SIDEBAR_MONO_SIZE_PX, SeanceWorkspace,
    forms::{CredentialEditorState, SettingsSection},
    perf::RedrawReason,
};

impl SeanceWorkspace {
    pub(crate) fn vault_unlocked(&self) -> bool {
        self.backend.vault_status().unlocked
    }

    pub(crate) fn refresh_vault_cache(&mut self) {
        self.cached_credentials = self.backend.list_password_credentials().unwrap_or_default();
        self.cached_keys = self.backend.list_private_keys().unwrap_or_default();
    }

    pub(crate) fn open_vault_panel(&mut self, cx: &mut Context<Self>) {
        self.open_settings_panel(SettingsSection::Vault, cx);
    }

    pub(crate) fn begin_edit_credential(&mut self, id: &str, cx: &mut Context<Self>) {
        match self.backend.load_password_credential(id) {
            Ok(Some(credential)) => {
                self.credential_editor = Some(CredentialEditorState::from_credential(credential));
            }
            Ok(None) => {
                self.show_toast("Credential not found.");
                self.refresh_vault_cache();
            }
            Err(err) => {
                self.show_toast(err.to_string());
            }
        }
        cx.notify();
    }

    pub(crate) fn save_credential_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self.credential_editor.as_ref() else {
            return;
        };
        let draft = VaultPasswordCredential {
            id: editor.credential_id.clone().unwrap_or_default(),
            label: editor.label.trim().to_string(),
            username_hint: (!editor.username_hint.trim().is_empty())
                .then(|| editor.username_hint.trim().to_string()),
            secret: editor.secret.clone(),
        };
        match self.backend.save_password_credential(draft) {
            Ok(summary) => {
                self.show_toast(format!("Saved credential '{}'.", summary.label));
                self.credential_editor = None;
                self.refresh_vault_cache();
            }
            Err(err) => {
                if let Some(editor) = self.credential_editor.as_mut() {
                    editor.message = Some(err.to_string());
                }
            }
        }
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    pub(crate) fn delete_credential(&mut self, id: &str, cx: &mut Context<Self>) {
        match self.backend.delete_password_credential(id) {
            Ok(true) => {
                self.show_toast("Credential deleted.");
                self.refresh_vault_cache();
            }
            Ok(false) => {
                self.show_toast("Credential already removed.");
            }
            Err(err) => {
                self.show_toast(err.to_string());
            }
        }
        cx.notify();
    }

    pub(crate) fn delete_private_key(&mut self, id: &str, cx: &mut Context<Self>) {
        match self.backend.delete_private_key(id) {
            Ok(true) => {
                self.show_toast("Key deleted.");
                self.refresh_vault_cache();
            }
            Ok(false) => {
                self.show_toast("Key already removed.");
            }
            Err(err) => {
                self.show_toast(err.to_string());
            }
        }
        cx.notify();
    }

    pub(crate) fn render_vault_section(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();

        let cred_count = self.cached_credentials.len();
        let key_count = self.cached_keys.len();
        let meta = format!("{} creds  {} keys", cred_count, key_count);

        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(self.render_sidebar_section_heading("vault", meta));

        let vault_action = |label: &'static str| {
            div()
                .px(px(14.0))
                .py(px(3.0))
                .cursor_pointer()
                .font_family(SIDEBAR_FONT_MONO)
                .text_size(px(SIDEBAR_MONO_SIZE_PX))
                .text_color(theme.text_ghost)
                .hover(|style| style.text_color(theme.text_secondary))
                .child(label)
        };

        section = section.child(
            div()
                .flex()
                .flex_col()
                .child(
                    vault_action(
                        if self.is_settings_panel_open()
                            && self.settings_panel.section == SettingsSection::Vault
                        {
                            "\u{25c9} manage vault"
                        } else {
                            "\u{25cb} manage vault"
                        },
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            if this.is_settings_panel_open()
                                && this.settings_panel.section == SettingsSection::Vault
                            {
                                this.close_settings_panel(cx);
                            } else {
                                this.open_vault_panel(cx);
                            }
                        }),
                    ),
                )
                .child(vault_action("+ add credential").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.credential_editor = Some(CredentialEditorState::blank());
                        this.perf_overlay.mark_input(RedrawReason::Input);
                        cx.notify();
                    }),
                ))
                .child(vault_action("+ generate ed25519").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        match this
                            .backend
                            .generate_ed25519_key(format!("ed25519-{}", crate::now_ui_suffix()))
                        {
                            Ok(summary) => {
                                this.show_toast(format!(
                                    "Generated key '{}'.",
                                    summary.label
                                ));
                                this.refresh_vault_cache();
                            }
                            Err(err) => this.show_toast(err.to_string()),
                        }
                        this.perf_overlay.mark_input(RedrawReason::Input);
                        cx.notify();
                    }),
                ))
                .child(vault_action("+ generate rsa").on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        match this
                            .backend
                            .generate_rsa_key(format!("rsa-{}", crate::now_ui_suffix()))
                        {
                            Ok(summary) => {
                                this.show_toast(format!(
                                    "Generated key '{}'.",
                                    summary.label
                                ));
                                this.refresh_vault_cache();
                            }
                            Err(err) => this.show_toast(err.to_string()),
                        }
                        this.perf_overlay.mark_input(RedrawReason::Input);
                        cx.notify();
                    }),
                )),
        );

        section
    }

    pub(crate) fn render_vault_credentials_card(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();

        let mut card = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_xl()
            .bg(theme.glass_tint)
            .border_1()
            .border_color(theme.glass_border);

        card = card.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.text_primary)
                                .child("Credentials"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(format!("{}", self.cached_credentials.len())),
                        ),
                )
                .child(
                    div()
                        .px_3()
                        .py(px(5.0))
                        .rounded_md()
                        .bg(theme.accent_glow)
                        .text_xs()
                        .text_color(theme.text_primary)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.accent))
                        .child("+ add credential")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.credential_editor = Some(CredentialEditorState::blank());
                                cx.notify();
                            }),
                        ),
                ),
        );

        if self.cached_credentials.is_empty() {
            card = card.child(
                div().py_4().flex().items_center().justify_center().child(
                    div()
                        .text_sm()
                        .text_color(theme.text_ghost)
                        .child("No password credentials stored"),
                ),
            );
        } else {
            card = card.child(div().h(px(1.0)).bg(theme.glass_border));

            let mut rows = div().flex().flex_col();
            for credential in &self.cached_credentials {
                rows = rows.child(self.render_credential_row(credential, cx));
            }
            card = card.child(rows);
        }

        card
    }

    fn render_credential_row(&self, credential: &CredentialSummary, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let cred_id = credential.id.clone();
        let cred_id_del = credential.id.clone();
        let hint = credential.username_hint.as_deref().unwrap_or("--");
        let truncated_id = if credential.id.len() > 8 {
            format!("{}...", &credential.id[..8])
        } else {
            credential.id.clone()
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .px_2()
            .py(px(6.0))
            .rounded_md()
            .hover(|style| style.bg(theme.sidebar_row_hover))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_primary)
                            .child(credential.label.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.text_muted)
                                    .child(hint.to_string()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_color(theme.text_ghost)
                                    .child(truncated_id),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py(px(3.0))
                            .rounded(px(4.0))
                            .text_xs()
                            .text_color(theme.text_ghost)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.text_secondary).bg(theme.glass_hover))
                            .child("edit")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.begin_edit_credential(&cred_id, cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py(px(3.0))
                            .rounded(px(4.0))
                            .text_xs()
                            .text_color(theme.text_ghost)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.warning).bg(theme.glass_hover))
                            .child("del")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.delete_credential(&cred_id_del, cx);
                                }),
                            ),
                    ),
            )
    }

    pub(crate) fn render_vault_keys_card(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();

        let mut card = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .rounded_xl()
            .bg(theme.glass_tint)
            .border_1()
            .border_color(theme.glass_border);

        card = card.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(theme.text_primary)
                                .child("SSH Keys"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.text_muted)
                                .child(format!("{}", self.cached_keys.len())),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(
                            div()
                                .px_3()
                                .py(px(5.0))
                                .rounded_md()
                                .bg(theme.accent_glow)
                                .text_xs()
                                .text_color(theme.text_primary)
                                .cursor_pointer()
                                .hover(|style| style.bg(theme.accent))
                                .child("+ ed25519")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        match this.backend.generate_ed25519_key(format!(
                                            "ed25519-{}",
                                            crate::now_ui_suffix()
                                        )) {
                                            Ok(summary) => {
                                                this.show_toast(format!(
                                                    "Generated key '{}'.",
                                                    summary.label
                                                ));
                                                this.refresh_vault_cache();
                                            }
                                            Err(err) => this.show_toast(err.to_string()),
                                        }
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .px_3()
                                .py(px(5.0))
                                .rounded_md()
                                .bg(theme.accent_glow)
                                .text_xs()
                                .text_color(theme.text_primary)
                                .cursor_pointer()
                                .hover(|style| style.bg(theme.accent))
                                .child("+ rsa-4096")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        match this
                                            .backend
                                            .generate_rsa_key(format!("rsa-{}", crate::now_ui_suffix()))
                                        {
                                            Ok(summary) => {
                                                this.show_toast(format!(
                                                    "Generated key '{}'.",
                                                    summary.label
                                                ));
                                                this.refresh_vault_cache();
                                            }
                                            Err(err) => this.show_toast(err.to_string()),
                                        }
                                        cx.notify();
                                    }),
                                ),
                        ),
                ),
        );

        if self.cached_keys.is_empty() {
            card = card.child(
                div().py_4().flex().items_center().justify_center().child(
                    div()
                        .text_sm()
                        .text_color(theme.text_ghost)
                        .child("No SSH keys stored"),
                ),
            );
        } else {
            card = card.child(div().h(px(1.0)).bg(theme.glass_border));

            let mut rows = div().flex().flex_col();
            for key in &self.cached_keys {
                rows = rows.child(self.render_key_row(key, cx));
            }
            card = card.child(rows);
        }

        card
    }

    fn render_key_row(&self, key: &KeySummary, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let key_id_del = key.id.clone();

        let algo_label = match &key.algorithm {
            PrivateKeyAlgorithm::Ed25519 => "ED25519".to_string(),
            PrivateKeyAlgorithm::Rsa { bits } => format!("RSA-{bits}"),
        };
        let source_label = match key.source {
            PrivateKeySource::Generated => "generated",
            PrivateKeySource::Imported => "imported",
        };
        let truncated_id = if key.id.len() > 8 {
            format!("{}...", &key.id[..8])
        } else {
            key.id.clone()
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .px_2()
            .py(px(6.0))
            .rounded_md()
            .hover(|style| style.bg(theme.sidebar_row_hover))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.text_primary)
                            .child(key.label.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                div()
                                    .px(px(5.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(theme.accent_glow)
                                    .text_xs()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_color(theme.accent)
                                    .child(algo_label),
                            )
                            .child(div().text_xs().text_color(theme.text_muted).child(source_label))
                            .child(
                                div()
                                    .text_xs()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_color(theme.text_ghost)
                                    .child(truncated_id),
                            ),
                    ),
            )
            .child(
                div().flex().items_center().gap_2().child(
                    div()
                        .px_2()
                        .py(px(3.0))
                        .rounded(px(4.0))
                        .text_xs()
                        .text_color(theme.text_ghost)
                        .cursor_pointer()
                        .hover(|style| style.text_color(theme.warning).bg(theme.glass_hover))
                        .child("del")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.delete_private_key(&key_id_del, cx);
                            }),
                        ),
                ),
            )
    }
}