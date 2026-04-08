// Owns non-render workspace coordination, input handling, config/update snapshots, and terminal state.

use std::{sync::Arc, time::Duration};

use gpui::{
    App, Context, FocusHandle, Focusable, KeyDownEvent, ScrollDelta, ScrollWheelEvent, Window,
    point, px,
};
use seance_config::AppConfig;
use seance_core::{UpdateState, VaultUiSnapshot};
use seance_terminal::{TerminalScreenKind, TerminalScrollCommand};
use seance_vault::{SecretString, UnlockMethod};

use crate::{
    app::{InitialWorkspaceAction, refresh_app_menus},
    forms::{
        CredentialDraftField, HostDraftField, SecureInputTarget, SecureSection, SettingsSection,
        TunnelDraftField, UnlockMode, VaultModalOrigin, WorkspaceSurface,
    },
    model::SeanceWorkspace,
    palette::{
        PageDirection, PaletteAction, PaletteViewModel, build_items, flatten_items,
        page_target_index,
    },
    perf::RedrawReason,
    surface::ShapeCache,
    theme::Theme,
    ui_components::{theme_id_from_config, update_status_label},
};

const SCOPE_KEY_SEPARATOR: &str = "::";

pub(crate) fn item_scope_key(vault_id: &str, item_id: &str) -> String {
    format!("{vault_id}{SCOPE_KEY_SEPARATOR}{item_id}")
}

pub(crate) fn host_scope_key(vault_id: &str, host_id: &str) -> String {
    item_scope_key(vault_id, host_id)
}

pub(crate) fn split_scope_key(scope_key: &str) -> Option<(&str, &str)> {
    scope_key.split_once(SCOPE_KEY_SEPARATOR)
}

impl SeanceWorkspace {
    fn current_vault_ui_snapshot(&self) -> VaultUiSnapshot {
        VaultUiSnapshot {
            managed_vaults: self.backend.list_vaults(),
            saved_hosts: self.backend.list_hosts().unwrap_or_default(),
            cached_credentials: self.backend.list_password_credentials().unwrap_or_default(),
            cached_keys: self.backend.list_private_keys().unwrap_or_default(),
            cached_port_forwards: self.backend.list_port_forwards().unwrap_or_default(),
        }
    }

    pub(crate) fn refresh_vault_ui(&mut self, cx: &mut Context<Self>) {
        self.apply_vault_snapshot(self.current_vault_ui_snapshot(), cx);
    }

    fn has_unlocked_vault(&self, vault_id: &str) -> bool {
        self.managed_vaults
            .iter()
            .any(|vault| vault.vault_id == vault_id && vault.unlocked)
    }

    pub(crate) fn saved_host_exists(&self, vault_id: &str, host_id: &str) -> bool {
        self.saved_hosts
            .iter()
            .any(|host| host.vault_id == vault_id && host.host.id == host_id)
    }

    pub(crate) fn saved_tunnel_exists(&self, vault_id: &str, tunnel_id: &str) -> bool {
        self.cached_port_forwards
            .iter()
            .any(|tunnel| tunnel.vault_id == vault_id && tunnel.port_forward.id == tunnel_id)
    }

    fn saved_credential_exists(&self, vault_id: &str, credential_id: &str) -> bool {
        self.cached_credentials.iter().any(|credential| {
            credential.vault_id == vault_id && credential.credential.id == credential_id
        })
    }

    fn saved_key_exists(&self, vault_id: &str, key_id: &str) -> bool {
        self.cached_keys
            .iter()
            .any(|key| key.vault_id == vault_id && key.key.id == key_id)
    }

    fn reconcile_saved_selection_state(&mut self) {
        if self.selected_host_id.as_deref().is_some_and(|scope_key| {
            split_scope_key(scope_key)
                .map(|(vault_id, host_id)| !self.saved_host_exists(vault_id, host_id))
                .unwrap_or(true)
        }) {
            self.selected_host_id = self
                .saved_hosts
                .first()
                .map(|host| host_scope_key(&host.vault_id, &host.host.id));
        }

        if self
            .secure
            .selected_host_id
            .as_deref()
            .is_some_and(|scope_key| {
                split_scope_key(scope_key)
                    .map(|(vault_id, host_id)| !self.saved_host_exists(vault_id, host_id))
                    .unwrap_or(true)
            })
        {
            self.secure.selected_host_id = None;
        }

        if self
            .secure
            .selected_tunnel_id
            .as_deref()
            .is_some_and(|scope_key| {
                split_scope_key(scope_key)
                    .map(|(vault_id, tunnel_id)| !self.saved_tunnel_exists(vault_id, tunnel_id))
                    .unwrap_or(true)
            })
        {
            self.secure.selected_tunnel_id = None;
        }

        if self
            .secure
            .selected_credential_id
            .as_deref()
            .is_some_and(|scope_key| {
                split_scope_key(scope_key)
                    .map(|(vault_id, credential_id)| {
                        !self.saved_credential_exists(vault_id, credential_id)
                    })
                    .unwrap_or(true)
            })
        {
            self.secure.selected_credential_id = None;
        }

        if self
            .secure
            .selected_key_id
            .as_deref()
            .is_some_and(|scope_key| {
                split_scope_key(scope_key)
                    .map(|(vault_id, key_id)| !self.saved_key_exists(vault_id, key_id))
                    .unwrap_or(true)
            })
        {
            self.secure.selected_key_id = None;
        }
    }

    fn reconcile_host_draft_after_vault_snapshot(&mut self) {
        let Some((vault_id, host_id)) = self
            .secure
            .host_draft
            .as_ref()
            .map(|draft| (draft.vault_id.clone(), draft.host_id.clone()))
        else {
            return;
        };
        let Some(vault_id) = vault_id else {
            return;
        };

        if !self.has_unlocked_vault(&vault_id) {
            self.secure.host_draft = None;
            self.secure.selected_host_id = None;
            self.show_toast("Host editor closed because its vault is no longer available.");
            return;
        }

        let Some(host_id) = host_id else {
            return;
        };

        if self.saved_host_exists(&vault_id, &host_id) {
            return;
        }

        let Some(draft) = self.secure.host_draft.as_mut() else {
            return;
        };
        draft.host_id = None;
        draft.dirty = true;
        draft.error = Some(
            "This saved item was deleted from the vault. Saving will create a new record.".into(),
        );
        self.secure.selected_host_id = None;
        self.show_toast("Saved host was deleted from the vault. Editing a new copy.");
    }

    fn reconcile_tunnel_draft_after_vault_snapshot(&mut self) {
        let Some((vault_id, tunnel_id)) = self
            .secure
            .tunnel_draft
            .as_ref()
            .map(|draft| (draft.vault_id.clone(), draft.port_forward_id.clone()))
        else {
            return;
        };
        let Some(vault_id) = vault_id else {
            return;
        };

        if !self.has_unlocked_vault(&vault_id) {
            self.secure.tunnel_draft = None;
            self.secure.selected_tunnel_id = None;
            self.show_toast("Tunnel editor closed because its vault is no longer available.");
            return;
        }

        let Some(tunnel_id) = tunnel_id else {
            return;
        };

        if self.saved_tunnel_exists(&vault_id, &tunnel_id) {
            return;
        }

        let Some(draft) = self.secure.tunnel_draft.as_mut() else {
            return;
        };
        draft.port_forward_id = None;
        draft.dirty = true;
        draft.error = Some(
            "This saved item was deleted from the vault. Saving will create a new record.".into(),
        );
        self.secure.selected_tunnel_id = None;
        self.show_toast("Saved tunnel was deleted from the vault. Editing a new copy.");
    }

    fn reconcile_credential_draft_after_vault_snapshot(&mut self) {
        let Some((vault_id, credential_id)) = self
            .secure
            .credential_draft
            .as_ref()
            .map(|draft| (draft.vault_id.clone(), draft.credential_id.clone()))
        else {
            return;
        };
        let Some(vault_id) = vault_id else {
            return;
        };

        if !self.has_unlocked_vault(&vault_id) {
            self.secure.credential_draft = None;
            self.secure.selected_credential_id = None;
            self.show_toast("Credential editor closed because its vault is no longer available.");
            return;
        }

        let Some(credential_id) = credential_id else {
            return;
        };

        if self.saved_credential_exists(&vault_id, &credential_id) {
            return;
        }

        let Some(draft) = self.secure.credential_draft.as_mut() else {
            return;
        };
        draft.credential_id = None;
        draft.dirty = true;
        draft.error = Some(
            "This saved item was deleted from the vault. Saving will create a new record.".into(),
        );
        self.secure.selected_credential_id = None;
        self.show_toast("Saved credential was deleted from the vault. Editing a new copy.");
    }

    pub(crate) fn handle_terminal_scroll_wheel(
        &mut self,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.active_session() else {
            return;
        };
        let summary = session.summary();
        if matches!(summary.active_screen, TerminalScreenKind::Alternate) || summary.mouse_tracking
        {
            return;
        }

        let line_height = self
            .terminal_metrics
            .map(|metrics| metrics.line_height_px)
            .unwrap_or_else(|| self.terminal_line_height_px());
        let delta_y = match event.delta {
            ScrollDelta::Pixels(delta) => f32::from(delta.y),
            ScrollDelta::Lines(delta) => delta.y * line_height,
        };
        let delta_rows = if delta_y.abs() < f32::EPSILON {
            0
        } else {
            (-(delta_y / line_height).round()) as isize
        };
        if delta_rows == 0 {
            return;
        }

        let _ = session.scroll_viewport(TerminalScrollCommand::DeltaRows(delta_rows));
        self.perf_overlay.mark_input(RedrawReason::TerminalUpdate);
        cx.notify();
    }

    fn handle_terminal_scrollback_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = self.active_session() else {
            return false;
        };
        let summary = session.summary();
        if matches!(summary.active_screen, TerminalScreenKind::Alternate) {
            return false;
        }

        let modifiers = event.keystroke.modifiers;
        if !modifiers.shift {
            return false;
        }

        let command = match event.keystroke.key.as_str() {
            "pageup" => Some(TerminalScrollCommand::PageUp),
            "pagedown" => Some(TerminalScrollCommand::PageDown),
            "home" => Some(TerminalScrollCommand::Top),
            "end" => Some(TerminalScrollCommand::Bottom),
            _ => None,
        };
        let Some(command) = command else {
            return false;
        };

        let result = match command {
            TerminalScrollCommand::Bottom => session.scroll_to_bottom(),
            _ => session.scroll_viewport(command),
        };
        if result.is_ok() {
            self.perf_overlay.mark_input(RedrawReason::TerminalUpdate);
            cx.notify();
        }
        true
    }

    pub(crate) fn apply_initial_action(
        &mut self,
        action: InitialWorkspaceAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            InitialWorkspaceAction::ConnectHost { vault_id, host_id } => {
                self.selected_host_id = Some(host_scope_key(&vault_id, &host_id));
                self.connect_saved_host(&vault_id, &host_id, window, cx);
            }
            InitialWorkspaceAction::CheckForUpdates => self.check_for_updates(cx),
            InitialWorkspaceAction::OpenPreferences => {
                self.open_settings_panel(SettingsSection::General, cx)
            }
            InitialWorkspaceAction::OpenCommandPalette => self.toggle_palette(cx),
            InitialWorkspaceAction::TogglePerfHud => self.toggle_perf_mode(window, cx),
        }
    }

    pub(crate) fn theme(&self) -> Theme {
        self.active_theme.theme()
    }

    pub(crate) fn terminal_font_size_px(&self) -> f32 {
        self.config.terminal.font_size_px
    }

    pub(crate) fn terminal_line_height_px(&self) -> f32 {
        self.config.terminal.line_height_px
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn apply_config_snapshot(
        &mut self,
        config: AppConfig,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_theme = self.active_theme;
        let previous_font_family = self.config.terminal.font_family.clone();
        let previous_font_size = self.config.terminal.font_size_px;
        let previous_line_height = self.config.terminal.line_height_px;

        self.config = config;
        self.active_theme = theme_id_from_config(&self.config);

        if self.active_theme != previous_theme {
            self.invalidate_terminal_surface();
            refresh_app_menus(cx);
        }

        if previous_font_family != self.config.terminal.font_family
            || (previous_font_size - self.config.terminal.font_size_px).abs() > f32::EPSILON
            || (previous_line_height - self.config.terminal.line_height_px).abs() > f32::EPSILON
        {
            self.terminal_metrics = None;
            self.terminal_surface.shape_cache = ShapeCache::default();
            self.invalidate_terminal_surface();
            self.apply_active_terminal_geometry(window);
        }

        if self.perf_mode_env_override.is_none() {
            self.apply_perf_mode(self.config.debug.perf_hud_default.into(), window, cx);
        }

        cx.notify();
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn apply_update_state_snapshot(
        &mut self,
        next_state: UpdateState,
        cx: &mut Context<Self>,
    ) {
        self.update_state = next_state.clone();
        match next_state {
            UpdateState::Available(ref update) => {
                self.show_toast(format!("Update {} is available.", update.version));
            }
            UpdateState::Checking
            | UpdateState::Downloading
            | UpdateState::Installing
            | UpdateState::ReadyToRelaunch
            | UpdateState::UpToDate => {
                self.show_toast(update_status_label(&self.update_state).to_string());
            }
            UpdateState::Failed(error) => {
                self.show_toast(error);
            }
            UpdateState::Idle => {}
        }
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn apply_tunnel_state_snapshot(
        &mut self,
        snapshots: Vec<seance_ssh::PortForwardRuntimeSnapshot>,
        cx: &mut Context<Self>,
    ) {
        self.active_port_forwards = snapshots;
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn apply_vault_snapshot(
        &mut self,
        snapshot: VaultUiSnapshot,
        cx: &mut Context<Self>,
    ) {
        self.managed_vaults = snapshot.managed_vaults;
        self.saved_hosts = snapshot.saved_hosts;
        self.cached_credentials = snapshot.cached_credentials;
        self.cached_keys = snapshot.cached_keys;
        self.cached_port_forwards = snapshot.cached_port_forwards;

        self.reconcile_saved_selection_state();
        self.reconcile_host_draft_after_vault_snapshot();
        self.reconcile_tunnel_draft_after_vault_snapshot();
        self.reconcile_credential_draft_after_vault_snapshot();

        refresh_app_menus(cx);
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn schedule_config_watcher(
        window: &mut Window,
        cx: &mut Context<Self>,
        entity: gpui::Entity<Self>,
        config_rx: std::sync::mpsc::Receiver<AppConfig>,
    ) {
        let config_rx = Arc::new(std::sync::Mutex::new(config_rx));
        window
            .spawn(cx, async move |cx| {
                loop {
                    let rx = Arc::clone(&config_rx);
                    let next_config = cx
                        .background_executor()
                        .spawn(async move { rx.lock().unwrap().recv().ok() })
                        .await;
                    let Some(mut next_config) = next_config else {
                        break;
                    };
                    while let Ok(config) = config_rx.lock().unwrap().try_recv() {
                        next_config = config;
                    }
                    let entity = entity.clone();
                    let _ = cx.update(move |window, cx| {
                        entity.update(cx, |this, cx| {
                            this.apply_config_snapshot(next_config, window, cx);
                        });
                        window.refresh();
                    });
                }
            })
            .detach();
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn schedule_vault_watcher(
        window: &mut Window,
        cx: &mut Context<Self>,
        entity: gpui::Entity<Self>,
        vault_rx: std::sync::mpsc::Receiver<VaultUiSnapshot>,
    ) {
        let vault_rx = Arc::new(std::sync::Mutex::new(vault_rx));
        window
            .spawn(cx, async move |cx| {
                loop {
                    let rx = Arc::clone(&vault_rx);
                    let next_snapshot = cx
                        .background_executor()
                        .spawn(async move { rx.lock().unwrap().recv().ok() })
                        .await;
                    let Some(mut next_snapshot) = next_snapshot else {
                        break;
                    };
                    while let Ok(snapshot) = vault_rx.lock().unwrap().try_recv() {
                        next_snapshot = snapshot;
                    }
                    let entity = entity.clone();
                    let _ = cx.update(move |window, cx| {
                        entity.update(cx, |this, cx| {
                            this.apply_vault_snapshot(next_snapshot, cx);
                        });
                        window.refresh();
                    });
                }
            })
            .detach();
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn schedule_update_watcher(
        window: &mut Window,
        cx: &mut Context<Self>,
        entity: gpui::Entity<Self>,
        update_rx: std::sync::mpsc::Receiver<UpdateState>,
    ) {
        let update_rx = Arc::new(std::sync::Mutex::new(update_rx));
        window
            .spawn(cx, async move |cx| {
                loop {
                    let rx = Arc::clone(&update_rx);
                    let next_state = cx
                        .background_executor()
                        .spawn(async move { rx.lock().unwrap().recv().ok() })
                        .await;
                    let Some(mut next_state) = next_state else {
                        break;
                    };
                    while let Ok(state) = update_rx.lock().unwrap().try_recv() {
                        next_state = state;
                    }
                    let entity = entity.clone();
                    let _ = cx.update(move |window, cx| {
                        entity.update(cx, |this, cx| {
                            this.apply_update_state_snapshot(next_state, cx);
                        });
                        window.refresh();
                    });
                }
            })
            .detach();
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn schedule_tunnel_state_watcher(
        window: &mut Window,
        cx: &mut Context<Self>,
        entity: gpui::Entity<Self>,
        tunnel_rx: std::sync::mpsc::Receiver<Vec<seance_ssh::PortForwardRuntimeSnapshot>>,
    ) {
        let tunnel_rx = Arc::new(std::sync::Mutex::new(tunnel_rx));
        window
            .spawn(cx, async move |cx| {
                loop {
                    let rx = Arc::clone(&tunnel_rx);
                    let next_state = cx
                        .background_executor()
                        .spawn(async move { rx.lock().unwrap().recv().ok() })
                        .await;
                    let Some(mut next_state) = next_state else {
                        break;
                    };
                    while let Ok(state) = tunnel_rx.lock().unwrap().try_recv() {
                        next_state = state;
                    }
                    let entity = entity.clone();
                    let _ = cx.update(move |_window, cx| {
                        entity.update(cx, |this, cx| {
                            this.apply_tunnel_state_snapshot(next_state, cx);
                        });
                    });
                }
            })
            .detach();
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn schedule_tunnel_animation(
        window: &mut Window,
        cx: &mut Context<Self>,
        entity: gpui::Entity<Self>,
    ) {
        window
            .spawn(cx, async move |cx| {
                loop {
                    cx.background_executor()
                        .spawn(async move {
                            std::thread::sleep(Duration::from_millis(250));
                        })
                        .await;
                    let _ = cx.update(|_window, cx| {
                        entity.update(cx, |this, cx| {
                            if !this.active_port_forwards.is_empty() {
                                cx.notify();
                            }
                        });
                    });
                }
            })
            .detach();
    }

    pub(crate) fn toggle_perf_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next_mode = self.perf_overlay.mode.next();
        self.apply_perf_mode(next_mode, window, cx);
    }

    pub(crate) fn open_vault_modal(
        &mut self,
        mode: UnlockMode,
        origin: VaultModalOrigin,
        message: String,
        cx: &mut Context<Self>,
    ) {
        self.open_vault_modal_for(None, mode, origin, message, cx);
    }

    pub(crate) fn open_vault_modal_for(
        &mut self,
        target_vault_id: Option<String>,
        mode: UnlockMode,
        origin: VaultModalOrigin,
        message: String,
        cx: &mut Context<Self>,
    ) {
        self.vault_modal.open(mode, origin, message);
        self.vault_modal.target_vault_id = target_vault_id.clone();
        if matches!(mode, UnlockMode::Create) {
            self.vault_modal.vault_name = format!("Vault {}", self.managed_vaults.len() + 1).into();
        } else if matches!(mode, UnlockMode::Rename)
            && let Some(vault) = target_vault_id.as_deref().and_then(|vault_id| {
                self.managed_vaults
                    .iter()
                    .find(|vault| vault.vault_id == vault_id)
            })
        {
            self.vault_modal.vault_name = vault.name.clone().into();
        }

        let device_unlock_available = target_vault_id
            .as_deref()
            .and_then(|vault_id| {
                self.managed_vaults
                    .iter()
                    .find(|vault| vault.vault_id == vault_id)
                    .map(|vault| vault.device_unlock_available)
            })
            .unwrap_or_else(|| self.backend.vault_status().device_unlock_available);

        if matches!(mode, UnlockMode::Unlock) && device_unlock_available {
            self.vault_modal.unlock_method = UnlockMethod::Device;
        }
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    pub(crate) fn submit_vault_modal(&mut self, cx: &mut Context<Self>) {
        self.vault_modal.error = None;
        self.vault_modal.busy = true;
        let target_vault_id = self.vault_modal.target_vault_id.clone().or_else(|| {
            self.managed_vaults
                .iter()
                .find(|vault| vault.open && !vault.unlocked)
                .or_else(|| self.managed_vaults.iter().find(|vault| !vault.unlocked))
                .map(|vault| vault.vault_id.clone())
        });

        match self.vault_modal.mode {
            UnlockMode::Create => {
                if !self.vault_modal.can_submit() {
                    self.vault_modal.error = Some(
                        "Choose a vault name and a non-empty matching recovery passphrase.".into(),
                    );
                } else {
                    let passphrase = SecretString::from(self.vault_modal.passphrase.to_string());
                    let next_name = self.vault_modal.vault_name.trim().to_string();
                    match self
                        .backend
                        .create_vault(next_name, &passphrase, "This Device")
                    {
                        Ok(summary) => {
                            self.vault_modal.close();
                            self.show_toast(if summary.device_unlock_message.is_some() {
                                "Encrypted vault created. Device unlock still needs attention."
                            } else {
                                "Encrypted vault created."
                            });
                            self.refresh_vault_ui(cx);
                        }
                        Err(err) => self.vault_modal.error = Some(err.to_string()),
                    }
                }
            }
            UnlockMode::Rename => {
                let next_name = self.vault_modal.vault_name.trim().to_string();
                match target_vault_id.as_deref() {
                    Some(vault_id) if !next_name.is_empty() => {
                        match self.backend.rename_vault(vault_id, next_name) {
                            Ok(_) => {
                                self.vault_modal.close();
                                self.show_toast("Vault renamed.");
                                self.refresh_vault_ui(cx);
                            }
                            Err(err) => self.vault_modal.error = Some(err.to_string()),
                        }
                    }
                    Some(_) => {
                        self.vault_modal.error = Some("Enter a vault name.".into());
                    }
                    None => {
                        self.vault_modal.error = Some("No vault is selected to rename.".into());
                    }
                }
            }
            UnlockMode::Unlock => match self.vault_modal.unlock_method {
                UnlockMethod::Device => match target_vault_id.as_deref() {
                    Some(vault_id) => match self.backend.try_unlock_with_device(vault_id) {
                        Ok(true) => {
                            self.vault_modal.close();
                            self.show_toast("Vault unlocked from device credentials.");
                            self.refresh_vault_ui(cx);
                        }
                        Ok(false) => {
                            self.vault_modal.unlock_method = UnlockMethod::Passphrase;
                            self.vault_modal.error = Some(
                                "Device unlock failed. Enter your recovery passphrase.".into(),
                            );
                        }
                        Err(err) => {
                            self.vault_modal.unlock_method = UnlockMethod::Passphrase;
                            self.vault_modal.error =
                                Some(format!("{}. Enter your recovery passphrase.", err));
                        }
                    },
                    None => {
                        self.vault_modal.error =
                            Some("No locked vault is available to unlock.".into())
                    }
                },
                UnlockMethod::Passphrase => {
                    if !self.vault_modal.can_submit() {
                        self.vault_modal.error =
                            Some("Enter the recovery passphrase to unlock the vault.".into());
                    } else {
                        let passphrase =
                            SecretString::from(self.vault_modal.passphrase.to_string());
                        match target_vault_id.as_deref() {
                            Some(vault_id) => {
                                match self.backend.unlock_vault(
                                    vault_id,
                                    &passphrase,
                                    "This Device",
                                ) {
                                    Ok(()) => {
                                        self.vault_modal.close();
                                        self.show_toast("Vault unlocked.");
                                        self.refresh_vault_ui(cx);
                                    }
                                    Err(err) => self.vault_modal.error = Some(err.to_string()),
                                }
                            }
                            None => {
                                self.vault_modal.error =
                                    Some("No locked vault is available to unlock.".into());
                            }
                        }
                    }
                }
            },
        }

        self.vault_modal.busy = false;
        self.perf_overlay.mark_input(RedrawReason::Input);
        refresh_app_menus(cx);
        cx.notify();
    }

    pub(crate) fn lock_vault(&mut self, cx: &mut Context<Self>) {
        for vault in self
            .managed_vaults
            .iter()
            .filter(|vault| vault.unlocked)
            .map(|vault| vault.vault_id.clone())
            .collect::<Vec<_>>()
        {
            let _ = self.backend.lock_vault(&vault);
        }
        self.refresh_vault_ui(cx);
        self.confirm_dialog = None;
        self.surface = WorkspaceSurface::Terminal;
        self.vault_modal.open(
            UnlockMode::Unlock,
            VaultModalOrigin::UserAction,
            "Vault locked. Decrypted records were cleared from memory.".into(),
        );
        self.show_toast("Vault locked.");
        self.palette_open = false;
        self.invalidate_terminal_surface();
        self.perf_overlay.mark_input(RedrawReason::Input);
        refresh_app_menus(cx);
        cx.notify();
    }

    pub(crate) fn toggle_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette_open {
            self.palette_open = false;
        } else {
            self.palette_open = true;
            self.palette_query.clear();
            self.palette_selected = 0;
            self.reset_palette_scroll_to_top();
        }
        self.perf_overlay.mark_input(RedrawReason::Palette);
        cx.notify();
    }

    pub(crate) fn palette_view_model(&self) -> PaletteViewModel {
        let session_labels = self.palette_session_labels();
        let remote_ids = self.remote_session_ids();
        let sessions = self.sessions();
        let items = build_items(
            &sessions,
            &session_labels,
            &self.saved_hosts,
            &self.connect_attempts.pending_summaries(),
            &self.cached_credentials,
            &self.cached_keys,
            &self.cached_port_forwards,
            &self.active_port_forwards,
            self.active_session_id,
            self.active_theme,
            &self.palette_query,
            self.vault_unlocked(),
            &remote_ids,
            &self.update_state,
        );

        flatten_items(items, self.palette_query.is_empty())
    }

    fn set_palette_selection(&mut self, new_index: usize, scroll_into_view: bool) {
        let view_model = self.palette_view_model();
        self.palette_selected = if view_model.items.is_empty() {
            0
        } else {
            new_index.min(view_model.items.len().saturating_sub(1))
        };

        if scroll_into_view {
            let row_index = view_model.item_to_row.get(self.palette_selected).copied();
            if let Some(row_index) = row_index {
                self.scroll_palette_selection_into_view(row_index);
            }
        }
    }

    fn scroll_palette_selection_into_view(&mut self, row_index: usize) {
        self.palette_scroll_handle.scroll_to_item(row_index);
    }

    fn reset_palette_scroll_to_top(&mut self) {
        self.palette_scroll_handle
            .set_offset(point(px(0.0), px(0.0)));
    }

    fn palette_visible_row_span(&self) -> usize {
        if self.palette_scroll_handle.children_count() == 0 {
            8
        } else {
            self.palette_scroll_handle
                .bottom_item()
                .saturating_sub(self.palette_scroll_handle.top_item())
                .max(1)
        }
    }

    fn move_palette_by_page(&mut self, direction: PageDirection) {
        let view_model = self.palette_view_model();
        if view_model.items.is_empty() {
            self.palette_selected = 0;
            return;
        }

        let next_index = page_target_index(
            &view_model.row_to_item,
            &view_model.item_to_row,
            self.palette_selected
                .min(view_model.items.len().saturating_sub(1)),
            self.palette_visible_row_span(),
            direction,
        );
        self.set_palette_selection(next_index, true);
    }

    pub(crate) fn execute_palette_action(
        &mut self,
        action: PaletteAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            PaletteAction::NewLocalTerminal => self.spawn_session(window, cx),
            PaletteAction::CheckForUpdates => {
                self.check_for_updates(cx);
            }
            PaletteAction::InstallAvailableUpdate => {
                self.install_available_update(cx);
            }
            PaletteAction::SwitchSession(id) => self.select_session(id, window, cx),
            PaletteAction::CloseActiveSession => {
                let id = self.active_session_id;
                self.close_session(id, window, cx);
            }
            PaletteAction::SwitchTheme(tid) => {
                self.persist_theme(tid, window, cx);
            }
            PaletteAction::UnlockVault => {
                self.open_vault_modal(
                    UnlockMode::Unlock,
                    VaultModalOrigin::UserAction,
                    "Enter the recovery passphrase to unlock the vault.".into(),
                    cx,
                );
            }
            PaletteAction::LockVault => {
                self.lock_vault(cx);
                return;
            }
            PaletteAction::AddSavedHost => {
                self.begin_add_host(cx);
                return;
            }
            PaletteAction::OpenVaultPanel => {
                self.open_vault_panel(cx);
                return;
            }
            PaletteAction::AddPasswordCredential => {
                self.open_secure_workspace(SecureSection::Credentials, cx);
                self.activate_credential_draft(
                    None,
                    crate::forms::CredentialDraftOrigin::Standalone,
                    cx,
                );
            }
            PaletteAction::EditPasswordCredential {
                vault_id,
                credential_id,
            } => {
                self.open_secure_workspace(SecureSection::Credentials, cx);
                self.activate_credential_draft(
                    Some(&item_scope_key(&vault_id, &credential_id)),
                    crate::forms::CredentialDraftOrigin::Standalone,
                    cx,
                );
                return;
            }
            PaletteAction::DeletePasswordCredential {
                vault_id,
                credential_id,
            } => {
                self.attempt_delete_credential(&item_scope_key(&vault_id, &credential_id), cx);
                return;
            }
            PaletteAction::ImportPrivateKey => {
                self.show_toast(
                    "Private key import backend is ready; UI import form is still pending.",
                );
            }
            PaletteAction::GenerateEd25519Key => {
                self.open_secure_workspace(SecureSection::Keys, cx);
                self.generate_ed25519_key_for_secure(cx);
            }
            PaletteAction::GenerateRsaKey => {
                self.open_secure_workspace(SecureSection::Keys, cx);
                self.generate_rsa_key_for_secure(cx);
            }
            PaletteAction::DeletePrivateKey { vault_id, key_id } => {
                self.attempt_delete_private_key(&item_scope_key(&vault_id, &key_id), cx);
                return;
            }
            PaletteAction::EditSavedHost { vault_id, host_id } => {
                self.begin_edit_host(&host_scope_key(&vault_id, &host_id), cx);
                return;
            }
            PaletteAction::DeleteSavedHost { vault_id, host_id } => {
                self.delete_saved_host(&host_scope_key(&vault_id, &host_id), cx);
                return;
            }
            PaletteAction::CancelSavedHostConnect { attempt_id } => {
                self.cancel_connect_attempt(attempt_id, cx);
                self.palette_open = false;
                self.palette_query.clear();
                self.palette_selected = 0;
                self.reset_palette_scroll_to_top();
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
                return;
            }
            PaletteAction::ConnectSavedHost { vault_id, host_id } => {
                self.start_connect_attempt(&vault_id, &host_id, window, cx);
                self.palette_open = false;
                self.palette_query.clear();
                self.palette_selected = 0;
                self.reset_palette_scroll_to_top();
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
                return;
            }
            PaletteAction::OpenSftpBrowser(session_id) => {
                self.open_sftp_browser(session_id, cx);
                return;
            }
            PaletteAction::OpenTunnelManager => {
                self.open_secure_workspace(SecureSection::Tunnels, cx);
                if self.secure.selected_tunnel_id.is_none() {
                    if let Some(first_tunnel) = self.cached_port_forwards.first() {
                        self.begin_edit_tunnel(
                            &item_scope_key(&first_tunnel.vault_id, &first_tunnel.port_forward.id),
                            cx,
                        );
                    } else {
                        self.begin_add_tunnel(cx);
                    }
                }
                return;
            }
            PaletteAction::OpenHostTunnelSettings { vault_id, host_id } => {
                self.activate_host_draft(Some(&host_scope_key(&vault_id, &host_id)), cx);
                return;
            }
            PaletteAction::StartTunnel {
                vault_id,
                port_forward_id,
            } => {
                self.start_saved_tunnel(&item_scope_key(&vault_id, &port_forward_id), cx);
                self.palette_open = false;
                self.palette_query.clear();
                self.palette_selected = 0;
                self.reset_palette_scroll_to_top();
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
                return;
            }
            PaletteAction::StopTunnel { tunnel_scope_key } => {
                self.stop_saved_tunnel(&tunnel_scope_key, cx);
                self.palette_open = false;
                self.palette_query.clear();
                self.palette_selected = 0;
                self.reset_palette_scroll_to_top();
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
                return;
            }
            PaletteAction::OpenPreferences => {
                self.open_settings_panel(SettingsSection::General, cx);
                return;
            }
        }
        self.perf_overlay.mark_input(RedrawReason::Palette);
        self.palette_open = false;
        self.palette_query.clear();
        self.palette_selected = 0;
        self.reset_palette_scroll_to_top();
        cx.notify();
    }

    pub(crate) fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();

        if self.confirm_dialog.is_some() {
            match key {
                "escape" => self.cancel_confirm_dialog(cx),
                "enter" => self.confirm_dialog_primary(cx),
                _ => {}
            }
            return;
        }
        if self.vault_modal.is_visible() {
            self.handle_vault_modal_key(event, cx);
            return;
        }

        if self.palette_open {
            self.handle_palette_key(event, window, cx);
            return;
        }

        if self.surface == WorkspaceSurface::Sftp && self.sftp_browser.is_some() {
            self.handle_sftp_key(event, window, cx);
            return;
        }

        if self.surface == WorkspaceSurface::Settings && key == "escape" {
            self.close_settings_panel(cx);
            self.perf_overlay.mark_input(RedrawReason::Input);
            return;
        }

        if self.surface == WorkspaceSurface::Secure {
            self.handle_secure_key(event, cx);
            return;
        }

        if self.handle_terminal_scrollback_key(event, cx) {
            return;
        }

        if let Some(bytes) = encode_keystroke(event)
            && let Some(session) = self.active_session()
        {
            let summary = session.summary();
            if matches!(summary.active_screen, TerminalScreenKind::Primary) && !summary.at_bottom {
                let _ = session.scroll_to_bottom();
            }
            let _ = session.send_input(bytes);
        }
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn handle_palette_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let key_char = event.keystroke.key_char.as_deref();

        match key {
            "escape" => {
                self.palette_open = false;
                self.palette_query.clear();
                self.palette_selected = 0;
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "up" => {
                self.set_palette_selection(self.palette_selected.saturating_sub(1), true);
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "down" => {
                self.set_palette_selection(self.palette_selected.saturating_add(1), true);
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "home" => {
                self.set_palette_selection(0, true);
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "end" => {
                let count = self.palette_view_model().items.len();
                if count > 0 {
                    self.set_palette_selection(count - 1, true);
                }
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "pageup" => {
                self.move_palette_by_page(PageDirection::Up);
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "pagedown" => {
                self.move_palette_by_page(PageDirection::Down);
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "enter" => {
                let view_model = self.palette_view_model();
                if let Some(item) = view_model.items.get(self.palette_selected) {
                    let action = item.action.clone();
                    self.execute_palette_action(action, window, cx);
                }
            }
            "backspace" => {
                self.palette_query.pop();
                self.palette_selected = 0;
                self.reset_palette_scroll_to_top();
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "tab" | "left" | "right" => {}
            _ => {
                if let Some(ch) = key_char {
                    let modifiers = event.keystroke.modifiers;
                    if !modifiers.platform && !modifiers.control && !modifiers.function {
                        self.palette_query.push_str(ch);
                        self.palette_selected = 0;
                        self.reset_palette_scroll_to_top();
                        self.perf_overlay.mark_input(RedrawReason::Palette);
                        cx.notify();
                    }
                }
            }
        }
    }

    fn handle_vault_modal_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let key_char = event.keystroke.key_char.as_deref();
        let modifiers = event.keystroke.modifiers;
        let field_count = self.vault_modal.passphrase_field_count();

        match key {
            "tab" | "down" => {
                if field_count > 0 {
                    self.vault_modal.selected_field =
                        (self.vault_modal.selected_field + 1) % field_count;
                }
            }
            "up" => {
                if field_count > 0 {
                    self.vault_modal.selected_field =
                        (self.vault_modal.selected_field + field_count - 1) % field_count;
                }
            }
            "backspace" => match self.vault_modal.mode {
                UnlockMode::Create => match self.vault_modal.selected_field {
                    0 => {
                        self.vault_modal.vault_name.pop();
                    }
                    1 => {
                        self.vault_modal.passphrase.pop();
                    }
                    _ if field_count > 1 => {
                        self.vault_modal.confirm_passphrase.pop();
                    }
                    _ => {}
                },
                UnlockMode::Rename => {
                    if self.vault_modal.selected_field == 0 {
                        self.vault_modal.vault_name.pop();
                    }
                }
                UnlockMode::Unlock => {
                    if self.vault_modal.selected_field == 0 {
                        self.vault_modal.passphrase.pop();
                    }
                }
            },
            "enter" => {
                self.submit_vault_modal(cx);
                return;
            }
            "escape" => {
                if self.vault_modal.can_close() {
                    self.vault_modal.close();
                    cx.notify();
                }
            }
            _ => {
                if let Some(ch) = key_char {
                    if !modifiers.platform && !modifiers.control && !modifiers.function {
                        match self.vault_modal.mode {
                            UnlockMode::Create => match self.vault_modal.selected_field {
                                0 => self.vault_modal.vault_name.push_str(ch),
                                1 => self.vault_modal.passphrase.push_str(ch),
                                _ if field_count > 1 => {
                                    self.vault_modal.confirm_passphrase.push_str(ch)
                                }
                                _ => {}
                            },
                            UnlockMode::Rename => {
                                if self.vault_modal.selected_field == 0 {
                                    self.vault_modal.vault_name.push_str(ch);
                                }
                            }
                            UnlockMode::Unlock => {
                                if self.vault_modal.selected_field == 0 {
                                    self.vault_modal.passphrase.push_str(ch);
                                }
                            }
                        }
                    }
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn handle_secure_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let key_char = event.keystroke.key_char.as_deref();
        let modifiers = event.keystroke.modifiers;

        if modifiers.platform && key == "s" {
            if self.secure.host_draft.is_some() {
                self.save_host_draft(cx);
            } else if self.secure.tunnel_draft.is_some() {
                self.save_tunnel_draft(cx);
            } else if self.secure.credential_draft.is_some() {
                self.save_credential_draft(cx);
            }
            return;
        }

        match key {
            "escape" => {
                self.close_secure_workspace(cx);
            }
            "tab" => {
                self.cycle_secure_focus(modifiers.shift);
            }
            "enter" => {
                if self.secure.host_draft.is_some() {
                    self.save_host_draft(cx);
                } else if self.secure.tunnel_draft.is_some() {
                    self.save_tunnel_draft(cx);
                } else if self.secure.credential_draft.is_some() {
                    self.save_credential_draft(cx);
                }
            }
            "backspace" => {
                self.backspace_secure_input();
            }
            _ => {
                if let Some(ch) = key_char {
                    if !modifiers.platform && !modifiers.control && !modifiers.function {
                        self.push_secure_input(ch);
                    }
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn cycle_secure_focus(&mut self, backward: bool) {
        self.secure.input_target = match self.secure.section {
            SecureSection::Hosts => {
                let Some(draft) = self.secure.host_draft.as_mut() else {
                    return;
                };
                let position = HostDraftField::ALL
                    .iter()
                    .position(|field| *field == draft.selected_field)
                    .unwrap_or(0);
                let next = if backward {
                    (position + HostDraftField::ALL.len() - 1) % HostDraftField::ALL.len()
                } else {
                    (position + 1) % HostDraftField::ALL.len()
                };
                draft.selected_field = HostDraftField::ALL[next];
                SecureInputTarget::HostDraft(draft.selected_field)
            }
            SecureSection::Credentials => {
                let Some(draft) = self.secure.credential_draft.as_mut() else {
                    return;
                };
                let position = CredentialDraftField::ALL
                    .iter()
                    .position(|field| *field == draft.selected_field)
                    .unwrap_or(0);
                let next = if backward {
                    (position + CredentialDraftField::ALL.len() - 1)
                        % CredentialDraftField::ALL.len()
                } else {
                    (position + 1) % CredentialDraftField::ALL.len()
                };
                draft.selected_field = CredentialDraftField::ALL[next];
                SecureInputTarget::CredentialDraft(draft.selected_field)
            }
            SecureSection::Tunnels => {
                let Some(draft) = self.secure.tunnel_draft.as_mut() else {
                    return;
                };
                let position = TunnelDraftField::ALL
                    .iter()
                    .position(|field| *field == draft.selected_field)
                    .unwrap_or(0);
                let next = if backward {
                    (position + TunnelDraftField::ALL.len() - 1) % TunnelDraftField::ALL.len()
                } else {
                    (position + 1) % TunnelDraftField::ALL.len()
                };
                draft.selected_field = TunnelDraftField::ALL[next];
                SecureInputTarget::TunnelDraft(draft.selected_field)
            }
            SecureSection::Keys => SecureInputTarget::KeySearch,
        };
    }

    fn backspace_secure_input(&mut self) {
        match self.secure.input_target {
            SecureInputTarget::HostSearch => {
                self.secure.host_search.pop();
            }
            SecureInputTarget::TunnelSearch => {
                self.secure.tunnel_search.pop();
            }
            SecureInputTarget::CredentialSearch => {
                self.secure.credential_search.pop();
            }
            SecureInputTarget::KeySearch => {
                self.secure.key_search.pop();
            }
            SecureInputTarget::HostDraft(field) => {
                if let Some(draft) = self.secure.host_draft.as_mut() {
                    match field {
                        HostDraftField::Label => draft.label.pop(),
                        HostDraftField::Hostname => draft.hostname.pop(),
                        HostDraftField::Username => draft.username.pop(),
                        HostDraftField::Port => draft.port.pop(),
                        HostDraftField::Notes => draft.notes.pop(),
                    };
                    draft.dirty = true;
                }
            }
            SecureInputTarget::CredentialDraft(field) => {
                if let Some(draft) = self.secure.credential_draft.as_mut() {
                    match field {
                        CredentialDraftField::Label => draft.label.pop(),
                        CredentialDraftField::UsernameHint => draft.username_hint.pop(),
                        CredentialDraftField::Secret => draft.secret.pop(),
                    };
                    draft.dirty = true;
                }
            }
            SecureInputTarget::TunnelDraft(field) => {
                if let Some(draft) = self.secure.tunnel_draft.as_mut() {
                    match field {
                        TunnelDraftField::Label => {
                            let _ = draft.label.pop();
                        }
                        TunnelDraftField::Mode => {}
                        TunnelDraftField::ListenAddress => {
                            let _ = draft.listen_address.pop();
                        }
                        TunnelDraftField::ListenPort => {
                            let _ = draft.listen_port.pop();
                        }
                        TunnelDraftField::TargetAddress => {
                            let _ = draft.target_address.pop();
                        }
                        TunnelDraftField::TargetPort => {
                            let _ = draft.target_port.pop();
                        }
                        TunnelDraftField::Notes => {
                            let _ = draft.notes.pop();
                        }
                    };
                    draft.dirty = true;
                }
            }
        }
    }

    fn push_secure_input(&mut self, text: &str) {
        match self.secure.input_target {
            SecureInputTarget::HostSearch => self.secure.host_search.push_str(text),
            SecureInputTarget::TunnelSearch => self.secure.tunnel_search.push_str(text),
            SecureInputTarget::CredentialSearch => self.secure.credential_search.push_str(text),
            SecureInputTarget::KeySearch => self.secure.key_search.push_str(text),
            SecureInputTarget::HostDraft(field) => {
                if let Some(draft) = self.secure.host_draft.as_mut() {
                    match field {
                        HostDraftField::Label => draft.label.push_str(text),
                        HostDraftField::Hostname => draft.hostname.push_str(text),
                        HostDraftField::Username => draft.username.push_str(text),
                        HostDraftField::Port => {
                            if text.chars().all(|value| value.is_ascii_digit()) {
                                draft.port.push_str(text);
                            }
                        }
                        HostDraftField::Notes => draft.notes.push_str(text),
                    }
                    draft.dirty = true;
                }
            }
            SecureInputTarget::CredentialDraft(field) => {
                if let Some(draft) = self.secure.credential_draft.as_mut() {
                    match field {
                        CredentialDraftField::Label => draft.label.push_str(text),
                        CredentialDraftField::UsernameHint => draft.username_hint.push_str(text),
                        CredentialDraftField::Secret => draft.secret.push_str(text),
                    }
                    draft.dirty = true;
                }
            }
            SecureInputTarget::TunnelDraft(field) => {
                if let Some(draft) = self.secure.tunnel_draft.as_mut() {
                    match field {
                        TunnelDraftField::Label => draft.label.push_str(text),
                        TunnelDraftField::Mode => {
                            if text.eq_ignore_ascii_case("l") {
                                draft.mode = seance_vault::PortForwardMode::Local;
                            } else if text.eq_ignore_ascii_case("r") {
                                draft.mode = seance_vault::PortForwardMode::Remote;
                            }
                        }
                        TunnelDraftField::ListenAddress => draft.listen_address.push_str(text),
                        TunnelDraftField::ListenPort => {
                            if text.chars().all(|value| value.is_ascii_digit()) {
                                draft.listen_port.push_str(text);
                            }
                        }
                        TunnelDraftField::TargetAddress => draft.target_address.push_str(text),
                        TunnelDraftField::TargetPort => {
                            if text.chars().all(|value| value.is_ascii_digit()) {
                                draft.target_port.push_str(text);
                            }
                        }
                        TunnelDraftField::Notes => draft.notes.push_str(text),
                    }
                    draft.dirty = true;
                }
            }
        }
    }

    pub(crate) fn delete_saved_host(&mut self, host_scope_key: &str, cx: &mut Context<Self>) {
        let Some((vault_id, host_id)) = split_scope_key(host_scope_key) else {
            self.show_toast("Saved host scope is invalid.");
            return;
        };

        match self.backend.delete_host(vault_id, host_id) {
            Ok(true) => {
                self.show_toast("Saved host removed.");
                self.refresh_vault_ui(cx);
            }
            Ok(false) => self.show_toast("Saved host already removed."),
            Err(err) => self.show_toast(err.to_string()),
        }
        refresh_app_menus(cx);
        cx.notify();
    }
}

impl Focusable for SeanceWorkspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn encode_keystroke(event: &KeyDownEvent) -> Option<Vec<u8>> {
    let key = &event.keystroke.key;
    let key_char = event.keystroke.key_char.as_deref();
    let modifiers = event.keystroke.modifiers;

    if modifiers.platform || modifiers.alt || modifiers.function {
        return None;
    }

    if modifiers.control && key.len() == 1 {
        let byte = key.as_bytes()[0].to_ascii_lowercase();
        if byte.is_ascii_lowercase() {
            return Some(vec![byte - b'a' + 1]);
        }
    }

    match key.as_str() {
        "enter" => Some(vec![b'\r']),
        "tab" => Some(vec![b'\t']),
        "backspace" => Some(vec![0x7f]),
        "escape" => Some(vec![0x1b]),
        "space" => Some(vec![b' ']),
        "up" => Some(b"\x1b[A".to_vec()),
        "down" => Some(b"\x1b[B".to_vec()),
        "right" => Some(b"\x1b[C".to_vec()),
        "left" => Some(b"\x1b[D".to_vec()),
        "home" => Some(b"\x1b[H".to_vec()),
        "end" => Some(b"\x1b[F".to_vec()),
        "pageup" => Some(b"\x1b[5~".to_vec()),
        "pagedown" => Some(b"\x1b[6~".to_vec()),
        _ => key_char.map(|text| text.as_bytes().to_vec()),
    }
}
