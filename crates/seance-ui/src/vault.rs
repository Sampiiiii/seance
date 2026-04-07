// Owns vault cache refresh, per-vault sidebar controls, and default vault selection.

use gpui::{Context, Div, MouseButton, div, prelude::*, px};
use seance_core::ManagedVaultSummary;

use crate::{
    SIDEBAR_FONT_MONO, SIDEBAR_MONO_SIZE_PX, SeanceWorkspace,
    forms::ConfirmDialogState,
    forms::{SecureSection, UnlockMode, VaultModalOrigin},
    perf::RedrawReason,
};

impl SeanceWorkspace {
    pub(crate) fn vault_unlocked(&self) -> bool {
        self.managed_vaults.iter().any(|vault| vault.unlocked)
    }

    pub(crate) fn refresh_managed_vaults(&mut self) {
        self.managed_vaults = self.backend.list_vaults();
    }

    pub(crate) fn refresh_vault_cache(&mut self) {
        self.refresh_managed_vaults();
        self.cached_credentials = self.backend.list_password_credentials().unwrap_or_default();
        self.cached_keys = self.backend.list_private_keys().unwrap_or_default();
    }

    pub(crate) fn default_target_vault_id(&self) -> Option<String> {
        if let Some(default_id) = self.config.vaults.default_target_vault_id.as_ref()
            && self
                .managed_vaults
                .iter()
                .any(|vault| vault.vault_id == *default_id && vault.unlocked)
        {
            return Some(default_id.clone());
        }

        let mut unlocked = self
            .managed_vaults
            .iter()
            .filter(|vault| vault.unlocked)
            .map(|vault| (vault.vault_id.clone(), vault.name.to_lowercase()))
            .collect::<Vec<_>>();
        unlocked.sort_by(|left, right| left.1.cmp(&right.1));
        unlocked.into_iter().map(|(vault_id, _)| vault_id).next()
    }

    pub(crate) fn open_vault_panel(&mut self, cx: &mut Context<Self>) {
        self.open_secure_workspace(SecureSection::Credentials, cx);
    }

    fn refresh_after_vault_change(&mut self, cx: &mut Context<Self>) {
        self.refresh_saved_hosts();
        self.refresh_vault_cache();
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    fn set_default_vault(&mut self, vault_id: &str, cx: &mut Context<Self>) {
        match self.backend.set_default_target_vault(vault_id) {
            Ok(()) => {
                self.show_toast("Default vault updated.");
                self.refresh_after_vault_change(cx);
            }
            Err(err) => self.show_toast(err.to_string()),
        }
    }

    fn open_managed_vault(&mut self, vault_id: &str, cx: &mut Context<Self>) {
        match self.backend.open_vault(vault_id) {
            Ok(()) => self.refresh_after_vault_change(cx),
            Err(err) => self.show_toast(err.to_string()),
        }
    }

    fn close_managed_vault(&mut self, vault_id: &str, cx: &mut Context<Self>) {
        match self.backend.close_vault(vault_id) {
            Ok(()) => self.refresh_after_vault_change(cx),
            Err(err) => self.show_toast(err.to_string()),
        }
    }

    fn lock_named_vault(&mut self, vault_id: &str, cx: &mut Context<Self>) {
        match self.backend.lock_vault(vault_id) {
            Ok(()) => {
                self.show_toast("Vault locked.");
                self.refresh_after_vault_change(cx);
            }
            Err(err) => self.show_toast(err.to_string()),
        }
    }

    pub(crate) fn delete_managed_vault(&mut self, vault_id: &str, cx: &mut Context<Self>) {
        match self.backend.delete_vault_permanently(vault_id) {
            Ok(()) => {
                self.show_toast("Vault deleted.");
                self.refresh_after_vault_change(cx);
            }
            Err(err) => self.show_toast(err.to_string()),
        }
    }

    fn confirm_delete_vault(&mut self, vault: &ManagedVaultSummary, cx: &mut Context<Self>) {
        self.confirm_dialog = Some(ConfirmDialogState::delete_vault(
            &vault.name,
            &vault.db_path.display().to_string(),
            vault.vault_id.clone(),
        ));
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    fn begin_unlock_vault(&mut self, vault_id: &str, cx: &mut Context<Self>) {
        let Some(vault_name) = self
            .managed_vaults
            .iter()
            .find(|vault| vault.vault_id == vault_id)
            .map(|vault| (vault.name.clone(), vault.open))
        else {
            self.show_toast("Vault not found.");
            return;
        };

        if !vault_name.1 {
            self.open_managed_vault(vault_id, cx);
        }

        self.open_vault_modal_for(
            Some(vault_id.to_string()),
            UnlockMode::Unlock,
            VaultModalOrigin::UserAction,
            format!("Unlock '{}' to access its encrypted records.", vault_name.0),
            cx,
        );
    }

    fn vault_status_label(vault: &ManagedVaultSummary) -> &'static str {
        if !vault.open {
            "closed"
        } else if vault.unlocked {
            "unlocked"
        } else {
            "locked"
        }
    }

    pub(crate) fn render_vault_section(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let meta = format!(
            "{} open  {} unlocked",
            self.managed_vaults.iter().filter(|vault| vault.open).count(),
            self.managed_vaults.iter().filter(|vault| vault.unlocked).count()
        );

        let sidebar_action = |label: &'static str| {
            div()
                .px(px(6.0))
                .py(px(2.0))
                .rounded_md()
                .cursor_pointer()
                .font_family(SIDEBAR_FONT_MONO)
                .text_size(px(SIDEBAR_MONO_SIZE_PX))
                .text_color(theme.text_ghost)
                .hover(|style| {
                    style
                        .text_color(theme.text_secondary)
                        .bg(theme.sidebar_action_hover)
                })
                .child(label)
        };

        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(self.render_sidebar_section_heading("vaults", meta));

        for vault in &self.managed_vaults {
            let is_default = self
                .config
                .vaults
                .default_target_vault_id
                .as_deref()
                .is_some_and(|default_id| default_id == vault.vault_id);
            let status = Self::vault_status_label(vault);
            let meta_line = if vault.unlocked {
                format!(
                    "{}  {}h {}c {}k",
                    status, vault.host_count, vault.credential_count, vault.key_count
                )
            } else {
                status.to_string()
            };
            let vault_id = vault.vault_id.clone();

            // Status dot color
            let dot_color = if vault.unlocked {
                theme.accent
            } else if vault.open {
                theme.warning
            } else {
                theme.text_ghost
            };

            // Primary action label (shown inline, right-aligned)
            let primary_action_label: &'static str = if !vault.open {
                "open"
            } else if !vault.unlocked {
                "unlock"
            } else {
                "lock"
            };

            section = section.child(
                div()
                    .flex()
                    .flex_col()
                    // Main vault row
                    .child(
                        self.sidebar_row_shell(false)
                            .child(
                                div()
                                    .w(px(8.0))
                                    .h(px(8.0))
                                    .rounded_full()
                                    .bg(dot_color)
                                    .flex_shrink_0(),
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
                                            .text_size(px(12.0))
                                            .font_weight(gpui::FontWeight::MEDIUM)
                                            .text_color(theme.text_primary)
                                            .line_clamp(1)
                                            .child(if is_default {
                                                format!("{} [default]", vault.name)
                                            } else {
                                                vault.name.clone()
                                            }),
                                    )
                                    .child(
                                        div()
                                            .font_family(SIDEBAR_FONT_MONO)
                                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                            .text_color(if vault.unlocked {
                                                theme.accent
                                            } else {
                                                theme.sidebar_meta
                                            })
                                            .line_clamp(1)
                                            .child(meta_line),
                                    ),
                            )
                            .child({
                                let vault_id = vault_id.clone();
                                sidebar_action(primary_action_label).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        if !this
                                            .managed_vaults
                                            .iter()
                                            .any(|v| v.vault_id == vault_id && v.open)
                                        {
                                            this.open_managed_vault(&vault_id, cx);
                                        } else if this
                                            .managed_vaults
                                            .iter()
                                            .any(|v| v.vault_id == vault_id && v.unlocked)
                                        {
                                            this.lock_named_vault(&vault_id, cx);
                                        } else {
                                            this.begin_unlock_vault(&vault_id, cx);
                                        }
                                    }),
                                )
                            }),
                    )
                    // Availability error (if any)
                    .when(vault.availability_error.is_some(), |col| {
                        col.child(
                            div()
                                .px(px(14.0))
                                .font_family(SIDEBAR_FONT_MONO)
                                .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                .text_color(theme.warning)
                                .line_clamp(2)
                                .child(vault.availability_error.clone().unwrap_or_default()),
                        )
                    })
                    // Secondary actions row (close, rename, make default, delete)
                    .child(
                        div()
                            .pl(px(30.0))
                            .pr(px(12.0))
                            .pb(px(2.0))
                            .flex()
                            .flex_wrap()
                            .gap(px(4.0))
                            .when(vault.open, |row| {
                                let vault_id = vault_id.clone();
                                row.child(sidebar_action("close").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.close_managed_vault(&vault_id, cx);
                                    }),
                                ))
                            })
                            .child({
                                let vault_id = vault_id.clone();
                                sidebar_action("rename").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.open_vault_modal_for(
                                            Some(vault_id.clone()),
                                            UnlockMode::Rename,
                                            VaultModalOrigin::UserAction,
                                            "Rename this vault.".into(),
                                            cx,
                                        );
                                    }),
                                )
                            })
                            .when(!is_default, |row| {
                                let vault_id = vault_id.clone();
                                row.child(sidebar_action("make default").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.set_default_vault(&vault_id, cx);
                                    }),
                                ))
                            })
                            .when(!vault.unlocked, |row| {
                                let vault = vault.clone();
                                row.child(sidebar_action("delete").on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.confirm_delete_vault(&vault, cx);
                                    }),
                                ))
                            }),
                    ),
            );
        }

        section = section
            .child(
                div().px(px(14.0)).pt(px(2.0)).child(
                    sidebar_action("+ new vault").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.open_vault_modal(
                                UnlockMode::Create,
                                VaultModalOrigin::UserAction,
                                "Create a named encrypted vault for this device.".into(),
                                cx,
                            );
                        }),
                    ),
                ),
            )
            .child(
                div().px(px(14.0)).child(
                    sidebar_action("manage secure data").on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.open_vault_panel(cx);
                        }),
                    ),
                ),
            );

        section
    }
}
