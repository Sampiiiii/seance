// Owns the secure workspace surface, host/credential drafts, key details, and confirm dialogs.

use gpui::{App, Context, Div, FontWeight, MouseButton, Window, div, prelude::*, px};
use seance_core::{VaultScopedCredentialSummary, VaultScopedHostSummary, VaultScopedKeySummary};
use seance_vault::{HostAuthRef, PrivateKeyAlgorithm, VaultHostProfile, VaultPasswordCredential};

use crate::{
    SIDEBAR_FONT_MONO, SeanceWorkspace,
    forms::{
        ConfirmDialogKind, ConfirmDialogState, CredentialDraftField, CredentialDraftOrigin,
        CredentialDraftState, HostDraftField, HostDraftState, PendingAction, SecureInputTarget,
        SecureSection, VaultModalOrigin, WorkspaceSurface,
    },
    perf::RedrawReason,
    refresh_app_menus,
    ui_components::{editor_field_card, masked_value, settings_action_chip},
    workspace::{item_scope_key, split_scope_key},
};

#[derive(Clone, Debug)]
struct HostReference {
    id: String,
    label: String,
}

impl SeanceWorkspace {
    pub(crate) fn open_secure_workspace(&mut self, section: SecureSection, cx: &mut Context<Self>) {
        self.surface = WorkspaceSurface::Secure;
        self.secure.section = section;
        self.secure.message = None;
        self.confirm_dialog = None;
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn close_secure_workspace(&mut self, cx: &mut Context<Self>) {
        self.request_pending_action(PendingAction::CloseSecureWorkspace, cx);
    }

    pub(crate) fn has_unsaved_secure_changes(&self) -> bool {
        self.secure
            .host_draft
            .as_ref()
            .is_some_and(|draft| draft.dirty)
            || self
                .secure
                .credential_draft
                .as_ref()
                .is_some_and(|draft| draft.dirty)
    }

    pub(crate) fn request_pending_action(
        &mut self,
        pending: PendingAction,
        cx: &mut Context<Self>,
    ) {
        if self.has_unsaved_secure_changes() {
            self.confirm_dialog = Some(ConfirmDialogState::discard_changes(pending));
            cx.notify();
            return;
        }

        self.apply_pending_action(pending, cx);
    }

    pub(crate) fn apply_pending_action(&mut self, pending: PendingAction, cx: &mut Context<Self>) {
        self.confirm_dialog = None;
        match pending {
            PendingAction::CloseSecureWorkspace => {
                self.surface = WorkspaceSurface::Terminal;
                self.secure.host_draft = None;
                self.secure.credential_draft = None;
                self.secure.message = None;
            }
            PendingAction::SwitchSecureSection(section) => {
                self.secure.section = section;
                self.secure.host_draft = None;
                self.secure.credential_draft = None;
                self.secure.message = None;
                self.secure.input_target = match section {
                    SecureSection::Hosts => SecureInputTarget::HostSearch,
                    SecureSection::Credentials => SecureInputTarget::CredentialSearch,
                    SecureSection::Keys => SecureInputTarget::KeySearch,
                };
            }
            PendingAction::OpenHostDraft(host_id) => {
                self.activate_host_draft(host_id.as_deref(), cx);
                return;
            }
            PendingAction::OpenCredentialDraft(id, origin) => {
                self.activate_credential_draft(id.as_deref(), origin, cx);
                return;
            }
        }
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn cancel_confirm_dialog(&mut self, cx: &mut Context<Self>) {
        self.confirm_dialog = None;
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    pub(crate) fn confirm_dialog_primary(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.confirm_dialog.clone() else {
            return;
        };

        match dialog.kind {
            ConfirmDialogKind::DiscardChanges(pending) => self.apply_pending_action(pending, cx),
            ConfirmDialogKind::BlockedDeletion {
                review_section,
                review_host_id,
            } => {
                self.confirm_dialog = None;
                self.open_secure_workspace(review_section, cx);
                if let Some(host_id) = review_host_id {
                    self.activate_host_draft(Some(&host_id), cx);
                }
            }
            ConfirmDialogKind::DeleteVault { vault_id } => {
                self.confirm_dialog = None;
                self.delete_managed_vault(&vault_id, cx);
            }
        }
    }

    pub(crate) fn activate_host_draft(&mut self, host_id: Option<&str>, cx: &mut Context<Self>) {
        self.surface = WorkspaceSurface::Secure;
        self.secure.section = SecureSection::Hosts;
        self.refresh_vault_cache();
        self.refresh_saved_hosts();
        self.secure.host_draft = if let Some(host_id) = host_id {
            let Some((vault_id, host_id)) = split_scope_key(host_id) else {
                self.show_toast("Saved host scope is invalid.");
                return;
            };
            match self.backend.load_host(vault_id, host_id) {
                Ok(Some(host)) => {
                    self.secure.selected_host_id = Some(item_scope_key(vault_id, &host.id));
                    let mut draft = HostDraftState::from_host(host);
                    draft.vault_id = Some(vault_id.to_string());
                    Some(draft)
                }
                Ok(None) => {
                    self.show_toast("Saved host not found.");
                    None
                }
                Err(err) => {
                    self.show_toast(err.to_string());
                    None
                }
            }
        } else {
            let Some(vault_id) = self.default_target_vault_id() else {
                self.show_toast("Unlock a vault before creating a saved host.");
                return;
            };
            self.secure.selected_host_id = None;
            let mut draft = HostDraftState::blank();
            draft.vault_id = Some(vault_id);
            Some(draft)
        };
        if self.secure.host_draft.is_some() {
            self.secure.input_target = SecureInputTarget::HostDraft(HostDraftField::Label);
        }
        self.secure.credential_draft = None;
        self.confirm_dialog = None;
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn activate_credential_draft(
        &mut self,
        credential_id: Option<&str>,
        origin: CredentialDraftOrigin,
        cx: &mut Context<Self>,
    ) {
        self.surface = WorkspaceSurface::Secure;
        self.secure.section = SecureSection::Credentials;
        self.refresh_vault_cache();
        self.secure.credential_draft = if let Some(credential_id) = credential_id {
            let Some((vault_id, credential_id)) = split_scope_key(credential_id) else {
                self.show_toast("Credential scope is invalid.");
                return;
            };
            match self
                .backend
                .load_password_credential(vault_id, credential_id)
            {
                Ok(Some(credential)) => {
                    self.secure.selected_credential_id =
                        Some(item_scope_key(vault_id, &credential.id));
                    let mut draft = CredentialDraftState::from_credential(credential, origin);
                    draft.vault_id = Some(vault_id.to_string());
                    Some(draft)
                }
                Ok(None) => {
                    self.show_toast("Credential not found.");
                    None
                }
                Err(err) => {
                    self.show_toast(err.to_string());
                    None
                }
            }
        } else {
            let target_vault_id = if origin == CredentialDraftOrigin::HostAuth {
                self.secure
                    .host_draft
                    .as_ref()
                    .and_then(|draft| draft.vault_id.clone())
                    .or_else(|| self.default_target_vault_id())
            } else {
                self.default_target_vault_id()
            };
            let Some(vault_id) = target_vault_id else {
                self.show_toast("Unlock a vault before creating a credential.");
                return;
            };
            self.secure.selected_credential_id = None;
            let mut draft = CredentialDraftState::blank(origin);
            draft.vault_id = Some(vault_id);
            Some(draft)
        };
        if self.secure.credential_draft.is_some() {
            self.secure.input_target =
                SecureInputTarget::CredentialDraft(CredentialDraftField::Label);
        }
        self.confirm_dialog = None;
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn begin_add_host(&mut self, cx: &mut Context<Self>) {
        if !self.vault_unlocked() {
            self.open_vault_modal(
                crate::forms::UnlockMode::Unlock,
                VaultModalOrigin::SecureAccess,
                "Unlock the vault before adding a saved host.".into(),
                cx,
            );
            return;
        }
        self.activate_host_draft(None, cx);
    }

    pub(crate) fn begin_edit_host(&mut self, host_id: &str, cx: &mut Context<Self>) {
        self.request_pending_action(PendingAction::OpenHostDraft(Some(host_id.into())), cx);
    }

    pub(crate) fn save_host_draft(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.secure.host_draft.as_ref() else {
            return;
        };
        let errors = draft.validation_errors();
        if !errors.is_empty() {
            if let Some(draft) = self.secure.host_draft.as_mut() {
                draft.error = Some(errors.join(" "));
            }
            cx.notify();
            return;
        }

        let host = VaultHostProfile {
            id: draft.host_id.clone().unwrap_or_default(),
            label: draft.label.trim().into(),
            hostname: draft.hostname.trim().into(),
            port: draft.parsed_port().unwrap_or(22),
            username: draft.username.trim().into(),
            notes: (!draft.notes.trim().is_empty()).then(|| draft.notes.trim().to_string()),
            auth_order: draft.auth_items.clone(),
        };

        let Some(vault_id) = draft.vault_id.clone() else {
            if let Some(draft) = self.secure.host_draft.as_mut() {
                draft.error = Some("Choose an unlocked vault before saving this host.".into());
            }
            cx.notify();
            return;
        };

        match self.backend.save_host(&vault_id, host) {
            Ok(summary) => {
                self.refresh_saved_hosts();
                self.secure.selected_host_id =
                    Some(item_scope_key(&summary.vault_id, &summary.host.id));
                if let Some(draft) = self.secure.host_draft.as_mut() {
                    draft.vault_id = Some(summary.vault_id.clone());
                    draft.host_id = Some(summary.host.id.clone());
                    draft.dirty = false;
                    draft.error = None;
                }
                self.show_toast(format!("Saved host '{}'.", summary.host.label));
            }
            Err(err) => {
                if let Some(draft) = self.secure.host_draft.as_mut() {
                    draft.error = Some(err.to_string());
                }
            }
        }
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        refresh_app_menus(cx);
        cx.notify();
    }

    pub(crate) fn add_auth_password(&mut self, credential_id: &str, cx: &mut Context<Self>) {
        let Some(draft) = self.secure.host_draft.as_mut() else {
            return;
        };
        let auth = HostAuthRef::Password {
            credential_id: credential_id.into(),
        };
        draft.auth_items.push(auth);
        draft.selected_auth = Some(draft.auth_items.len().saturating_sub(1));
        draft.dirty = true;
        draft.error = None;
        cx.notify();
    }

    pub(crate) fn add_auth_key(&mut self, key_id: &str, cx: &mut Context<Self>) {
        let Some(draft) = self.secure.host_draft.as_mut() else {
            return;
        };
        draft.auth_items.push(HostAuthRef::PrivateKey {
            key_id: key_id.into(),
            passphrase_credential_id: None,
        });
        draft.selected_auth = Some(draft.auth_items.len().saturating_sub(1));
        draft.dirty = true;
        draft.error = None;
        cx.notify();
    }

    pub(crate) fn remove_auth_item(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(draft) = self.secure.host_draft.as_mut() else {
            return;
        };
        if index >= draft.auth_items.len() {
            return;
        }
        draft.auth_items.remove(index);
        draft.selected_auth = draft.selected_auth.and_then(|selected| {
            if draft.auth_items.is_empty() {
                None
            } else {
                Some(selected.min(draft.auth_items.len() - 1))
            }
        });
        draft.dirty = true;
        cx.notify();
    }

    pub(crate) fn move_auth_item(
        &mut self,
        index: usize,
        direction: isize,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = self.secure.host_draft.as_mut() else {
            return;
        };
        let target = if direction.is_negative() {
            index.saturating_sub(direction.unsigned_abs())
        } else {
            index.saturating_add(direction as usize)
        };
        if index >= draft.auth_items.len() || target >= draft.auth_items.len() {
            return;
        }
        draft.auth_items.swap(index, target);
        draft.selected_auth = Some(target);
        draft.dirty = true;
        cx.notify();
    }

    pub(crate) fn set_auth_passphrase_credential(
        &mut self,
        index: usize,
        credential_id: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = self.secure.host_draft.as_mut() else {
            return;
        };
        let Some(HostAuthRef::PrivateKey {
            passphrase_credential_id,
            ..
        }) = draft.auth_items.get_mut(index)
        else {
            return;
        };
        *passphrase_credential_id = credential_id;
        draft.dirty = true;
        cx.notify();
    }

    pub(crate) fn open_quick_create_credential(&mut self, cx: &mut Context<Self>) {
        self.request_pending_action(
            PendingAction::OpenCredentialDraft(None, CredentialDraftOrigin::HostAuth),
            cx,
        );
    }

    pub(crate) fn save_credential_draft(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.secure.credential_draft.as_ref() else {
            return;
        };
        let errors = draft.validation_errors();
        if !errors.is_empty() {
            if let Some(draft) = self.secure.credential_draft.as_mut() {
                draft.error = Some(errors.join(" "));
            }
            cx.notify();
            return;
        }

        let credential = VaultPasswordCredential {
            id: draft.credential_id.clone().unwrap_or_default(),
            label: draft.label.trim().to_string(),
            username_hint: (!draft.username_hint.trim().is_empty())
                .then(|| draft.username_hint.trim().to_string()),
            secret: draft.secret.clone(),
        };

        let Some(vault_id) = draft.vault_id.clone() else {
            if let Some(draft) = self.secure.credential_draft.as_mut() {
                draft.error =
                    Some("Choose an unlocked vault before saving this credential.".into());
            }
            cx.notify();
            return;
        };

        match self.backend.save_password_credential(&vault_id, credential) {
            Ok(summary) => {
                let origin = draft.origin.clone();
                self.refresh_vault_cache();
                self.secure.selected_credential_id =
                    Some(item_scope_key(&summary.vault_id, &summary.credential.id));
                self.show_toast(format!("Saved credential '{}'.", summary.credential.label));
                match origin {
                    CredentialDraftOrigin::Standalone => {
                        if let Some(draft) = self.secure.credential_draft.as_mut() {
                            draft.vault_id = Some(summary.vault_id.clone());
                            draft.credential_id = Some(summary.credential.id.clone());
                            draft.dirty = false;
                            draft.error = None;
                        }
                    }
                    CredentialDraftOrigin::HostAuth => {
                        self.secure.credential_draft = None;
                        self.secure.section = SecureSection::Hosts;
                        if let Some(host_draft) = self.secure.host_draft.as_mut() {
                            if host_draft.vault_id.as_deref() != Some(summary.vault_id.as_str()) {
                                self.show_toast(
                                    "Credentials can only be attached to hosts in the same vault.",
                                );
                                cx.notify();
                                return;
                            }
                            host_draft.auth_items.push(HostAuthRef::Password {
                                credential_id: summary.credential.id.clone(),
                            });
                            host_draft.selected_auth =
                                Some(host_draft.auth_items.len().saturating_sub(1));
                            host_draft.dirty = true;
                        }
                        self.secure.input_target = self
                            .secure
                            .host_draft
                            .as_ref()
                            .map(|draft| SecureInputTarget::HostDraft(draft.selected_field))
                            .unwrap_or(SecureInputTarget::HostSearch);
                    }
                }
            }
            Err(err) => {
                if let Some(draft) = self.secure.credential_draft.as_mut() {
                    draft.error = Some(err.to_string());
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn generate_ed25519_key_for_secure(&mut self, cx: &mut Context<Self>) {
        let Some(vault_id) = self
            .secure
            .host_draft
            .as_ref()
            .and_then(|draft| draft.vault_id.clone())
            .or_else(|| self.default_target_vault_id())
        else {
            self.show_toast("Unlock a vault before generating a key.");
            return;
        };

        match self
            .backend
            .generate_ed25519_key(&vault_id, format!("ed25519-{}", crate::now_ui_suffix()))
        {
            Ok(summary) => {
                self.refresh_vault_cache();
                self.secure.selected_key_id =
                    Some(item_scope_key(&summary.vault_id, &summary.key.id));
                self.show_toast(format!("Generated key '{}'.", summary.key.label));
            }
            Err(err) => self.show_toast(err.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn generate_rsa_key_for_secure(&mut self, cx: &mut Context<Self>) {
        let Some(vault_id) = self
            .secure
            .host_draft
            .as_ref()
            .and_then(|draft| draft.vault_id.clone())
            .or_else(|| self.default_target_vault_id())
        else {
            self.show_toast("Unlock a vault before generating a key.");
            return;
        };

        match self
            .backend
            .generate_rsa_key(&vault_id, format!("rsa-{}", crate::now_ui_suffix()))
        {
            Ok(summary) => {
                self.refresh_vault_cache();
                self.secure.selected_key_id =
                    Some(item_scope_key(&summary.vault_id, &summary.key.id));
                self.show_toast(format!("Generated key '{}'.", summary.key.label));
            }
            Err(err) => self.show_toast(err.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn attempt_delete_credential(
        &mut self,
        credential_id: &str,
        cx: &mut Context<Self>,
    ) {
        let Some((vault_id, credential_id)) = split_scope_key(credential_id) else {
            self.show_toast("Credential scope is invalid.");
            return;
        };

        let refs = self.host_references_for_credential(vault_id, credential_id);
        if !refs.is_empty() {
            self.confirm_dialog = Some(ConfirmDialogState::blocked_deletion(
                "credential",
                &refs
                    .iter()
                    .map(|item| item.label.clone())
                    .collect::<Vec<_>>(),
                SecureSection::Hosts,
                refs.first().map(|item| item.id.clone()),
            ));
            cx.notify();
            return;
        }

        match self
            .backend
            .delete_password_credential(vault_id, credential_id)
        {
            Ok(true) => {
                self.refresh_vault_cache();
                if self
                    .secure
                    .selected_credential_id
                    .as_deref()
                    .is_some_and(|selected| selected == item_scope_key(vault_id, credential_id))
                {
                    self.secure.selected_credential_id = None;
                }
                self.secure.credential_draft = None;
                self.show_toast("Credential deleted.");
            }
            Ok(false) => self.show_toast("Credential already removed."),
            Err(err) => self.show_toast(err.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn attempt_delete_private_key(&mut self, key_id: &str, cx: &mut Context<Self>) {
        let Some((vault_id, key_id)) = split_scope_key(key_id) else {
            self.show_toast("Key scope is invalid.");
            return;
        };

        let refs = self.host_references_for_key(vault_id, key_id);
        if !refs.is_empty() {
            self.confirm_dialog = Some(ConfirmDialogState::blocked_deletion(
                "key",
                &refs
                    .iter()
                    .map(|item| item.label.clone())
                    .collect::<Vec<_>>(),
                SecureSection::Hosts,
                refs.first().map(|item| item.id.clone()),
            ));
            cx.notify();
            return;
        }

        match self.backend.delete_private_key(vault_id, key_id) {
            Ok(true) => {
                self.refresh_vault_cache();
                if self
                    .secure
                    .selected_key_id
                    .as_deref()
                    .is_some_and(|selected| selected == item_scope_key(vault_id, key_id))
                {
                    self.secure.selected_key_id = None;
                }
                self.show_toast("Key deleted.");
            }
            Ok(false) => self.show_toast("Key already removed."),
            Err(err) => self.show_toast(err.to_string()),
        }
        cx.notify();
    }

    fn host_references_for_credential(
        &self,
        vault_id: &str,
        credential_id: &str,
    ) -> Vec<HostReference> {
        self.saved_hosts
            .iter()
            .filter(|summary| summary.vault_id == vault_id)
            .filter_map(|summary| {
                let Ok(Some(host)) = self.backend.load_host(&summary.vault_id, &summary.host.id)
                else {
                    return None;
                };
                let referenced = host.auth_order.iter().any(|auth| match auth {
                    HostAuthRef::Password { credential_id: id } => id == credential_id,
                    HostAuthRef::PrivateKey {
                        passphrase_credential_id,
                        ..
                    } => passphrase_credential_id.as_deref() == Some(credential_id),
                });
                referenced.then(|| HostReference {
                    id: item_scope_key(&summary.vault_id, &summary.host.id),
                    label: format!("{} [{}]", summary.host.label, summary.vault_name),
                })
            })
            .collect()
    }

    fn host_references_for_key(&self, vault_id: &str, key_id: &str) -> Vec<HostReference> {
        self.saved_hosts
            .iter()
            .filter(|summary| summary.vault_id == vault_id)
            .filter_map(|summary| {
                let Ok(Some(host)) = self.backend.load_host(&summary.vault_id, &summary.host.id)
                else {
                    return None;
                };
                host.auth_order
                    .iter()
                    .any(|auth| matches!(auth, HostAuthRef::PrivateKey { key_id: id, .. } if id == key_id))
                    .then(|| HostReference {
                        id: item_scope_key(&summary.vault_id, &summary.host.id),
                        label: format!("{} [{}]", summary.host.label, summary.vault_name),
                    })
            })
            .collect()
    }

    fn host_matches_query(&self, host: &VaultScopedHostSummary) -> bool {
        let query = self.secure.host_search.trim().to_lowercase();
        query.is_empty()
            || host.host.label.to_lowercase().contains(&query)
            || host.host.hostname.to_lowercase().contains(&query)
            || host.host.username.to_lowercase().contains(&query)
            || host.vault_name.to_lowercase().contains(&query)
    }

    fn credential_matches_query(&self, credential: &VaultScopedCredentialSummary) -> bool {
        let query = self.secure.credential_search.trim().to_lowercase();
        query.is_empty()
            || credential.credential.label.to_lowercase().contains(&query)
            || credential
                .credential
                .username_hint
                .as_deref()
                .unwrap_or_default()
                .to_lowercase()
                .contains(&query)
            || credential.vault_name.to_lowercase().contains(&query)
    }

    fn key_matches_query(&self, key: &VaultScopedKeySummary) -> bool {
        let query = self.secure.key_search.trim().to_lowercase();
        query.is_empty()
            || key.key.label.to_lowercase().contains(&query)
            || key.vault_name.to_lowercase().contains(&query)
    }

    pub(crate) fn render_secure_workspace(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme();

        let body = div()
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
            .on_key_down(cx.listener(Self::handle_key_down))
            .p_5()
            .flex()
            .flex_col()
            .gap_4()
            .child(self.render_secure_header(cx))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .gap_4()
                    .child(self.render_secure_list_panel(cx))
                    .child(self.render_secure_detail_panel(cx)),
            );

        body
    }

    fn render_secure_header(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let mut tabs = div().flex().items_center().gap(px(8.0));
        for section in SecureSection::ALL {
            let active = section == self.secure.section;
            tabs = tabs.child(
                div()
                    .px(px(12.0))
                    .py(px(6.0))
                    .rounded_full()
                    .border_1()
                    .border_color(if active { t.accent } else { t.glass_border })
                    .bg(if active {
                        t.accent_glow
                    } else {
                        gpui::transparent_black()
                    })
                    .text_sm()
                    .text_color(if active {
                        t.text_primary
                    } else {
                        t.text_secondary
                    })
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.request_pending_action(
                                PendingAction::SwitchSecureSection(section),
                                cx,
                            );
                        }),
                    )
                    .child(section.title()),
            );
        }

        div()
            .flex()
            .items_end()
            .justify_between()
            .gap_4()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_size(px(22.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(t.text_primary)
                            .child("Secure Workspace"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_muted)
                            .child(self.secure.section.subtitle()),
                    )
                    .child(tabs),
            )
            .child(settings_action_chip("back to terminal", &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.close_secure_workspace(cx);
                }),
            ))
    }

    fn render_secure_list_panel(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let mut panel = div()
            .w(px(320.0))
            .h_full()
            .p_4()
            .rounded_xl()
            .bg(t.glass_tint)
            .border_1()
            .border_color(t.glass_border)
            .flex()
            .flex_col()
            .gap_3();

        match self.secure.section {
            SecureSection::Hosts => {
                panel = panel
                    .child(
                        self.render_search_card(
                            "Search hosts",
                            &self.secure.host_search,
                            self.secure.input_target == SecureInputTarget::HostSearch,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.secure.input_target = SecureInputTarget::HostSearch;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(settings_action_chip("new host", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.request_pending_action(PendingAction::OpenHostDraft(None), cx);
                        }),
                    ));
                let mut rows = div().flex().flex_col().gap(px(6.0));
                for host in self
                    .saved_hosts
                    .iter()
                    .filter(|host| self.host_matches_query(host))
                {
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
                                host.host.username,
                                host.host.hostname,
                                host.host.port,
                                host.vault_name
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
                panel = panel.child(div().flex_1().child(rows));
            }
            SecureSection::Credentials => {
                panel = panel
                    .child(
                        self.render_search_card(
                            "Search credentials",
                            &self.secure.credential_search,
                            self.secure.input_target == SecureInputTarget::CredentialSearch,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.secure.input_target = SecureInputTarget::CredentialSearch;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(settings_action_chip("new credential", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.request_pending_action(
                                PendingAction::OpenCredentialDraft(
                                    None,
                                    CredentialDraftOrigin::Standalone,
                                ),
                                cx,
                            );
                        }),
                    ));
                let mut rows = div().flex().flex_col().gap(px(6.0));
                for credential in self
                    .cached_credentials
                    .iter()
                    .filter(|credential| self.credential_matches_query(credential))
                {
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
                                    .unwrap_or("Stored password"),
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
                panel = panel.child(div().flex_1().child(rows));
            }
            SecureSection::Keys => {
                panel = panel
                    .child(
                        self.render_search_card(
                            "Search keys",
                            &self.secure.key_search,
                            self.secure.input_target == SecureInputTarget::KeySearch,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.secure.input_target = SecureInputTarget::KeySearch;
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(settings_action_chip("ed25519", &t).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.generate_ed25519_key_for_secure(cx);
                                }),
                            ))
                            .child(settings_action_chip("rsa-4096", &t).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.generate_rsa_key_for_secure(cx);
                                }),
                            )),
                    );
                let mut rows = div().flex().flex_col().gap(px(6.0));
                for key in self
                    .cached_keys
                    .iter()
                    .filter(|key| self.key_matches_query(key))
                {
                    let key_scope_key = item_scope_key(&key.vault_id, &key.key.id);
                    let selected =
                        self.secure.selected_key_id.as_deref() == Some(key_scope_key.as_str());
                    let algo = match key.key.algorithm {
                        PrivateKeyAlgorithm::Ed25519 => "ed25519",
                        PrivateKeyAlgorithm::Rsa { .. } => "rsa",
                    };
                    rows = rows.child(
                        self.render_list_row(
                            &key.key.label,
                            &format!("{algo}  [{}]", key.vault_name),
                            selected,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.secure.selected_key_id = Some(key_scope_key.clone());
                                cx.notify();
                            }),
                        ),
                    );
                }
                panel = panel.child(div().flex_1().child(rows));
            }
        }

        panel
    }

    fn render_secure_detail_panel(&self, cx: &mut Context<Self>) -> Div {
        match self.secure.section {
            SecureSection::Hosts => self.render_hosts_detail(cx),
            SecureSection::Credentials => self.render_credentials_detail(cx),
            SecureSection::Keys => self.render_keys_detail(cx),
        }
    }

    fn render_hosts_detail(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let Some(draft) = self.secure.host_draft.as_ref() else {
            return self.render_placeholder_panel(
                "Select a host",
                "Choose a saved host from the list or create a new one.",
            );
        };

        let mut panel = div()
            .flex_1()
            .h_full()
            .p_5()
            .rounded_xl()
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
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
                                    .text_sm()
                                    .text_color(t.text_muted)
                                    .child("Build the connection profile and exact auth order."),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_color(if draft.dirty { t.warning } else { t.accent })
                            .child(if draft.dirty { "unsaved" } else { "saved" }),
                    ),
            );

        if let Some(error) = draft.error.as_ref() {
            panel = panel.child(self.render_banner(error, true));
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

        panel = panel
            .child(
                self.render_section_card(
                    "Connection",
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(label_field)
                        .child(hostname_field)
                        .child(username_field)
                        .child(port_field),
                ),
            )
            .child(self.render_section_card("Authentication", auth_builder))
            .child(self.render_section_card("Notes", div().child(notes_field)))
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child("Tab cycles fields. Enter saves when the form is valid."),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(8.0))
                            .child(settings_action_chip("discard", &t).on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.request_pending_action(
                                        PendingAction::OpenHostDraft(None),
                                        cx,
                                    );
                                }),
                            ))
                            .child(
                                div()
                                    .px(px(14.0))
                                    .py(px(7.0))
                                    .rounded_md()
                                    .bg(if draft.can_save() {
                                        t.accent_glow
                                    } else {
                                        t.glass_active
                                    })
                                    .text_sm()
                                    .text_color(t.text_primary)
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.save_host_draft(cx);
                                        }),
                                    )
                                    .child("save host"),
                            ),
                    ),
            );

        panel
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

        let mut selected = div().flex().flex_col().gap(px(8.0));
        if draft.auth_items.is_empty() {
            selected = selected.child(
                div()
                    .text_sm()
                    .text_color(t.text_muted)
                    .child("No authentication methods selected yet."),
            );
        } else {
            for (index, auth) in draft.auth_items.iter().enumerate() {
                let (label, passphrase_id) = match auth {
                    HostAuthRef::Password { credential_id } => (
                        format!(
                            "Password: {}",
                            available_credentials
                                .iter()
                                .find(|item| item.credential.id == *credential_id)
                                .map(|item| item.credential.label.as_str())
                                .unwrap_or("unknown")
                        ),
                        None,
                    ),
                    HostAuthRef::PrivateKey {
                        key_id,
                        passphrase_credential_id,
                    } => (
                        format!(
                            "Key: {}",
                            available_keys
                                .iter()
                                .find(|item| item.key.id == *key_id)
                                .map(|item| item.key.label.as_str())
                                .unwrap_or("unknown")
                        ),
                        passphrase_credential_id.clone(),
                    ),
                };
                let is_selected = draft.selected_auth == Some(index);
                let move_up = index > 0;
                let move_down = index + 1 < draft.auth_items.len();
                let mut card = div()
                    .p_3()
                    .rounded_lg()
                    .border_1()
                    .border_color(if is_selected {
                        t.accent
                    } else {
                        t.glass_border
                    })
                    .bg(if is_selected {
                        t.accent_glow
                    } else {
                        t.glass_tint
                    })
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
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t.text_primary)
                                    .child(label),
                            )
                            .child(
                                div()
                                    .flex()
                                    .gap(px(8.0))
                                    .child(settings_action_chip("remove", &t).on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.remove_auth_item(index, cx);
                                        }),
                                    ))
                                    .when(move_up, |row| {
                                        row.child(settings_action_chip("up", &t).on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.move_auth_item(index, -1, cx);
                                            }),
                                        ))
                                    })
                                    .when(move_down, |row| {
                                        row.child(settings_action_chip("down", &t).on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.move_auth_item(index, 1, cx);
                                            }),
                                        ))
                                    }),
                            ),
                    );

                if matches!(auth, HostAuthRef::PrivateKey { .. }) {
                    let mut row = div().flex().flex_wrap().gap(px(8.0)).items_center();
                    let passphrase_selection = passphrase_id.clone();
                    row = row.child(
                        settings_action_chip(
                            if passphrase_id.is_none() {
                                "passphrase: none"
                            } else {
                                "clear passphrase"
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
                                .px(px(10.0))
                                .py(px(5.0))
                                .rounded_full()
                                .border_1()
                                .border_color(if active { t.accent } else { t.glass_border })
                                .bg(if active {
                                    t.accent_glow
                                } else {
                                    gpui::transparent_black()
                                })
                                .text_xs()
                                .text_color(t.text_secondary)
                                .cursor_pointer()
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
                    card = card.child(
                        div()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child("Passphrase credential"),
                    );
                    card = card.child(row);
                }

                selected = selected.child(card);
            }
        }

        let mut available = div().flex().flex_col().gap(px(8.0));
        for credential in &available_credentials {
            let credential_id = credential.credential.id.clone();
            available = available.child(
                self.render_list_row(
                    &format!("Password: {}", credential.credential.label),
                    credential
                        .credential
                        .username_hint
                        .as_deref()
                        .unwrap_or("password credential"),
                    false,
                )
                .child(settings_action_chip("add", &t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.add_auth_password(&credential_id, cx);
                    }),
                )),
            );
        }
        for key in &available_keys {
            let key_id = key.key.id.clone();
            available = available.child(
                self.render_list_row(
                    &format!("Key: {}", key.key.label),
                    match key.key.algorithm {
                        PrivateKeyAlgorithm::Ed25519 => "ed25519",
                        PrivateKeyAlgorithm::Rsa { .. } => "rsa-4096",
                    },
                    false,
                )
                .child(settings_action_chip("add", &t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.add_auth_key(&key_id, cx);
                    }),
                )),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .flex()
                    .gap(px(8.0))
                    .child(settings_action_chip("new credential", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.open_quick_create_credential(cx);
                        }),
                    ))
                    .child(settings_action_chip("generate ed25519", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.generate_ed25519_key_for_secure(cx);
                        }),
                    ))
                    .child(settings_action_chip("generate rsa-4096", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.generate_rsa_key_for_secure(cx);
                        }),
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text_ghost)
                    .child("Selected auth sequence"),
            )
            .child(selected)
            .child(div().h(px(1.0)).bg(t.glass_border))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text_ghost)
                    .child("Available methods"),
            )
            .child(available)
    }

    fn render_credentials_detail(&self, cx: &mut Context<Self>) -> Div {
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

        let mut panel = div()
            .flex_1()
            .h_full()
            .p_5()
            .rounded_xl()
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.0))
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
                                    .text_sm()
                                    .text_color(t.text_muted)
                                    .child("Passwords are stored encrypted in the vault."),
                            ),
                    )
                    .child(
                        settings_action_chip(
                            if draft.reveal_secret {
                                "hide password"
                            } else {
                                "show password"
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
            panel = panel.child(self.render_banner(error, true));
        }

        let delete_button = draft.credential_id.as_ref().map(|credential_id| {
            let credential_scope_key = draft
                .vault_id
                .as_ref()
                .map(|vault_id| item_scope_key(vault_id, credential_id));
            settings_action_chip("delete credential", &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    if let Some(credential_scope_key) = credential_scope_key.as_ref() {
                        this.attempt_delete_credential(credential_scope_key, cx);
                    }
                }),
            )
        });

        let mut actions = div().flex().gap(px(8.0));
        if let Some(delete_button) = delete_button {
            actions = actions.child(delete_button);
        }

        panel.child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .child(actions)
                .child(
                    div()
                        .px(px(14.0))
                        .py(px(7.0))
                        .rounded_md()
                        .bg(if draft.can_save() {
                            t.accent_glow
                        } else {
                            t.glass_active
                        })
                        .text_sm()
                        .text_color(t.text_primary)
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.save_credential_draft(cx);
                            }),
                        )
                        .child("save credential"),
                ),
        )
    }

    fn render_keys_detail(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let Some(key_scope_key) = self.secure.selected_key_id.as_ref() else {
            return self.render_placeholder_panel(
                "Select a key",
                "Choose a generated key to inspect its algorithm and host references.",
            );
        };

        let Some((vault_id, key_id)) = split_scope_key(key_scope_key) else {
            return self
                .render_placeholder_panel("Missing key", "The selected key scope is invalid.");
        };

        let Some(key) = self
            .cached_keys
            .iter()
            .find(|key| key.vault_id == vault_id && key.key.id == key_id)
        else {
            return self.render_placeholder_panel(
                "Missing key",
                "The selected key is no longer available.",
            );
        };

        let refs = self.host_references_for_key(vault_id, key_id);
        let algorithm = match key.key.algorithm {
            PrivateKeyAlgorithm::Ed25519 => "Ed25519".to_string(),
            PrivateKeyAlgorithm::Rsa { bits } => format!("RSA-{bits}"),
        };

        div()
            .flex_1()
            .h_full()
            .p_5()
            .rounded_xl()
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text_primary)
                    .child(format!("{} [{}]", key.key.label, key.vault_name)),
            )
            .child(
                self.render_section_card(
                    "Key Metadata",
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .text_color(t.text_secondary)
                                .child(format!("Algorithm: {algorithm}")),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(t.text_secondary)
                                .child(format!("Source: {:?}", key.key.source)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(t.text_secondary)
                                .child(format!("Encrypted at rest: {}", key.key.encrypted_at_rest)),
                        ),
                ),
            )
            .child(self.render_section_card(
                "Host Usage",
                if refs.is_empty() {
                    div()
                        .text_sm()
                        .text_color(t.text_muted)
                        .child("This key is not referenced by any saved host.")
                } else {
                    let mut rows = div().flex().flex_col().gap(px(6.0));
                    for host in refs {
                        let host_id = host.id.clone();
                        rows = rows.child(
                            self.render_list_row(&host.label, "saved host", false)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.activate_host_draft(Some(&host_id), cx);
                                    }),
                                ),
                        );
                    }
                    rows
                },
            ))
            .child(settings_action_chip("delete key", &t).on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let key_scope_key = key_scope_key.clone();
                    move |this, _, _, cx| {
                        this.attempt_delete_private_key(&key_scope_key, cx);
                    }
                }),
            ))
    }

    fn render_search_card(&self, label: &'static str, value: &str, selected: bool) -> Div {
        let t = self.theme();
        editor_field_card(label, value.to_string(), selected, &t)
    }

    fn render_list_row(&self, title: &str, subtitle: &str, selected: bool) -> Div {
        let t = self.theme();
        div()
            .p_3()
            .rounded_lg()
            .border_1()
            .border_color(if selected { t.accent } else { t.glass_border })
            .bg(if selected {
                t.accent_glow
            } else {
                t.glass_tint
            })
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.text_primary)
                            .child(title.to_string()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child(subtitle.to_string()),
                    ),
            )
    }

    fn render_section_card(&self, title: &'static str, content: impl IntoElement) -> Div {
        let t = self.theme();
        div()
            .p_4()
            .rounded_xl()
            .bg(t.glass_tint)
            .border_1()
            .border_color(t.glass_border)
            .flex()
            .flex_col()
            .gap_3()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(t.text_ghost)
                    .child(title),
            )
            .child(content)
    }

    fn render_banner(&self, text: &str, warning: bool) -> Div {
        let t = self.theme();
        div()
            .px_4()
            .py(px(10.0))
            .rounded_lg()
            .border_1()
            .border_color(if warning { t.warning } else { t.glass_border })
            .bg(t.glass_tint)
            .text_sm()
            .text_color(if warning { t.warning } else { t.text_secondary })
            .child(text.to_string())
    }

    fn render_placeholder_panel(&self, title: &str, subtitle: &str) -> Div {
        let t = self.theme();
        div()
            .flex_1()
            .h_full()
            .p_5()
            .rounded_xl()
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_3()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text_primary)
                    .child(title.to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(t.text_muted)
                    .child(subtitle.to_string()),
            )
    }

    pub(crate) fn render_confirm_dialog(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let Some(dialog) = self.confirm_dialog.as_ref() else {
            return div();
        };

        div()
            .absolute()
            .size_full()
            .bg(t.scrim)
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(px(520.0))
                    .p_5()
                    .rounded_xl()
                    .bg(t.glass_strong)
                    .border_1()
                    .border_color(t.glass_border_bright)
                    .flex()
                    .flex_col()
                    .gap_4()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text_primary)
                            .child(dialog.title.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.text_muted)
                            .child(dialog.message.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .justify_end()
                            .gap(px(8.0))
                            .child(
                                settings_action_chip(dialog.cancel_label.clone(), &t)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.cancel_confirm_dialog(cx);
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .px(px(14.0))
                                    .py(px(7.0))
                                    .rounded_md()
                                    .bg(t.accent_glow)
                                    .text_sm()
                                    .text_color(t.text_primary)
                                    .cursor_pointer()
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.confirm_dialog_primary(cx);
                                        }),
                                    )
                                    .child(dialog.confirm_label.clone()),
                            ),
                    ),
            )
    }
}
