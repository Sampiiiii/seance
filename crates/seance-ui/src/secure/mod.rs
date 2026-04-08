// Owns the secure workspace surface, host/credential drafts, key details, and confirm dialogs.

mod credentials;
mod hosts;
mod keys;
mod shared;

use gpui::{App, Context, Div, FontWeight, MouseButton, Window, div, prelude::*, px};
use seance_core::{VaultScopedCredentialSummary, VaultScopedHostSummary, VaultScopedKeySummary};
use seance_vault::{HostAuthRef, VaultHostProfile, VaultPasswordCredential};

use crate::{
    SeanceWorkspace,
    forms::{
        ConfirmDialogKind, ConfirmDialogState, CredentialDraftField, CredentialDraftOrigin,
        CredentialDraftState, HostDraftField, HostDraftState, PendingAction, SecureInputTarget,
        SecureSection, VaultModalOrigin, WorkspaceSurface,
    },
    perf::RedrawReason,
    refresh_app_menus,
    ui_components::{danger_button, primary_button, settings_action_chip},
    workspace::{item_scope_key, split_scope_key},
};

// ── Referential-integrity helper ──────────────────────────────────────────

#[derive(Clone, Debug)]
pub(crate) struct HostReference {
    pub id: String,
    pub label: String,
}

// ── Business logic ───────────────────────────────────────────────────────

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
                .tunnel_draft
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
                self.secure.tunnel_draft = None;
                self.secure.credential_draft = None;
                self.secure.message = None;
            }
            PendingAction::SwitchSecureSection(section) => {
                self.secure.section = section;
                self.secure.host_draft = None;
                self.secure.tunnel_draft = None;
                self.secure.credential_draft = None;
                self.secure.message = None;
                self.secure.input_target = match section {
                    SecureSection::Hosts => SecureInputTarget::HostSearch,
                    SecureSection::Tunnels => SecureInputTarget::TunnelSearch,
                    SecureSection::Credentials => SecureInputTarget::CredentialSearch,
                    SecureSection::Keys => SecureInputTarget::KeySearch,
                };
            }
            PendingAction::OpenHostDraft(host_id) => {
                self.activate_host_draft(host_id.as_deref(), cx);
                return;
            }
            PendingAction::OpenTunnelDraft {
                tunnel_id,
                host_scope_key,
            } => {
                self.activate_tunnel_draft(tunnel_id.as_deref(), host_scope_key.as_deref(), cx);
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
        self.secure.tunnel_draft = None;
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
        self.secure.tunnel_draft = None;
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
                self.refresh_vault_ui(cx);
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
                self.refresh_vault_ui(cx);
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
                self.refresh_vault_ui(cx);
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
                self.refresh_vault_ui(cx);
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
                self.refresh_vault_ui(cx);
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
                self.refresh_vault_ui(cx);
                self.show_toast("Key deleted.");
            }
            Ok(false) => self.show_toast("Key already removed."),
            Err(err) => self.show_toast(err.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn host_references_for_credential(
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

    pub(crate) fn host_references_for_key(
        &self,
        vault_id: &str,
        key_id: &str,
    ) -> Vec<HostReference> {
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

    pub(crate) fn host_matches_query(&self, host: &VaultScopedHostSummary) -> bool {
        let query = self.secure.host_search.trim().to_lowercase();
        query.is_empty()
            || host.host.label.to_lowercase().contains(&query)
            || host.host.hostname.to_lowercase().contains(&query)
            || host.host.username.to_lowercase().contains(&query)
            || host.vault_name.to_lowercase().contains(&query)
    }

    pub(crate) fn credential_matches_query(
        &self,
        credential: &VaultScopedCredentialSummary,
    ) -> bool {
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

    pub(crate) fn key_matches_query(&self, key: &VaultScopedKeySummary) -> bool {
        let query = self.secure.key_search.trim().to_lowercase();
        query.is_empty()
            || key.key.label.to_lowercase().contains(&query)
            || key.vault_name.to_lowercase().contains(&query)
    }
}

// ── Render: top-level dispatch ───────────────────────────────────────────

impl SeanceWorkspace {
    pub(crate) fn render_secure_workspace(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme();

        div()
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
                    .min_h_0()
                    .flex()
                    .gap_4()
                    .child(self.render_secure_list_panel(cx))
                    .child(self.render_secure_detail_panel(cx)),
            )
    }

    fn render_secure_header(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();

        let item_counts = [
            ("Hosts", self.saved_hosts.len()),
            ("Tunnels", self.cached_port_forwards.len()),
            ("Credentials", self.cached_credentials.len()),
            ("Keys", self.cached_keys.len()),
        ];

        let mut tabs = div().flex().items_center().gap(px(6.0));
        for (i, section) in SecureSection::ALL.iter().enumerate() {
            let active = *section == self.secure.section;
            let count = item_counts[i].1;
            let label = if count > 0 {
                format!("{} {}", section.title(), count)
            } else {
                section.title().to_string()
            };
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
                    .hover(|s| if active { s } else { s.bg(t.glass_hover) })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.request_pending_action(
                                PendingAction::SwitchSecureSection(*section),
                                cx,
                            );
                        }),
                    )
                    .child(label),
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
            .child(settings_action_chip("← terminal", &t).on_mouse_down(
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
                    .child(settings_action_chip("+ new host", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.request_pending_action(PendingAction::OpenHostDraft(None), cx);
                        }),
                    ))
                    .child(
                        div()
                            .id("host-list-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(self.render_host_list_content(cx)),
                    );
            }
            SecureSection::Tunnels => {
                panel = self.render_tunnel_list_panel(cx);
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
                    .child(settings_action_chip("+ new credential", &t).on_mouse_down(
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
                    ))
                    .child(
                        div()
                            .id("credential-list-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(self.render_credential_list_content(cx)),
                    );
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
                            .gap(px(6.0))
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
                            )),
                    )
                    .child(
                        div()
                            .id("key-list-scroll")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(self.render_key_list_content(cx)),
                    );
            }
        }

        panel
    }

    fn render_secure_detail_panel(&self, cx: &mut Context<Self>) -> Div {
        match self.secure.section {
            SecureSection::Hosts => self.render_hosts_detail(cx),
            SecureSection::Tunnels => self.render_tunnel_detail_panel(cx),
            SecureSection::Credentials => self.render_credentials_detail(cx),
            SecureSection::Keys => self.render_keys_detail(cx),
        }
    }

    pub(crate) fn render_confirm_dialog(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let Some(dialog) = self.confirm_dialog.as_ref() else {
            return div();
        };

        let is_destructive = matches!(dialog.kind, ConfirmDialogKind::DeleteVault { .. });

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
                            .child(if is_destructive {
                                danger_button(dialog.confirm_label.clone(), &t).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        this.confirm_dialog_primary(cx);
                                    }),
                                )
                            } else {
                                primary_button(dialog.confirm_label.clone(), true, &t)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.confirm_dialog_primary(cx);
                                        }),
                                    )
                            }),
                    ),
            )
    }
}
