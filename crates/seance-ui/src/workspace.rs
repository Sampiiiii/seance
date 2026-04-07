// Owns non-render workspace coordination, input handling, config/update snapshots, and terminal state.

use std::{sync::Arc, time::Instant};

use gpui::{App, Context, FocusHandle, Focusable, KeyDownEvent, Window, font, point, px};
use seance_config::AppConfig;
use seance_core::UpdateState;
use seance_terminal::TerminalGeometry;
use seance_vault::{SecretString, UnlockMethod};
use tracing::trace;

use crate::{
    app::{InitialWorkspaceAction, refresh_app_menus},
    forms::{
        CredentialDraftField, HostDraftField, SecureInputTarget, SecureSection, SettingsSection,
        UnlockMode, VaultModalOrigin, WorkspaceSurface,
    },
    model::{SeanceWorkspace, TerminalMetrics, TerminalRendererMetrics},
    palette::{
        PageDirection, PaletteAction, PaletteViewModel, build_items, flatten_items,
        page_target_index,
    },
    perf::{RedrawReason, UiPerfMode},
    surface::ShapeCache,
    terminal_paint::build_terminal_surface_rows,
    theme::Theme,
    ui_components::{compute_terminal_geometry, theme_id_from_config, update_status_label},
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
            self.perf_overlay.mode = self.config.debug.perf_hud_default.into();
        }

        cx.notify();
    }

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

    pub(crate) fn terminal_metrics(&mut self, window: &Window) -> TerminalMetrics {
        if let Some(metrics) = self.terminal_metrics {
            return metrics;
        }

        let font_family = self.config.terminal.font_family.clone();
        let font_size_px = self.terminal_font_size_px();
        let line_height_px = self.terminal_line_height_px();
        let font_size = px(font_size_px);
        let font_id = window.text_system().resolve_font(&font(font_family));
        let cell_width_px = window
            .text_system()
            .ch_advance(font_id, font_size)
            .map(f32::from)
            .unwrap_or(8.0)
            .ceil()
            .max(1.0);
        let line_height_px = line_height_px.ceil().max(1.0);
        let metrics = TerminalMetrics {
            cell_width_px,
            cell_height_px: line_height_px,
            line_height_px,
            font_size_px,
        };
        trace!(?metrics, "measured terminal metrics");
        self.terminal_metrics = Some(metrics);
        metrics
    }

    pub(crate) fn apply_active_terminal_geometry(&mut self, window: &Window) {
        let Some(session) = self.active_session() else {
            self.last_applied_geometry = None;
            self.active_terminal_rows = TerminalGeometry::default().size.rows as usize;
            return;
        };

        let metrics = self.terminal_metrics(window);
        let geometry =
            compute_terminal_geometry(window.viewport_size(), metrics, self.sidebar_width)
                .unwrap_or_else(TerminalGeometry::default);
        self.active_terminal_rows = geometry.size.rows as usize;

        if self.last_applied_geometry == Some(geometry) {
            trace!(?geometry, "skipping unchanged UI terminal geometry");
            return;
        }

        trace!(
            ?geometry,
            session_id = session.id(),
            "computed UI terminal geometry"
        );
        if let Err(error) = session.resize(geometry) {
            trace!(
                ?geometry,
                session_id = session.id(),
                error = %error,
                "failed to apply terminal geometry"
            );
            return;
        }

        self.last_applied_geometry = Some(geometry);
        self.invalidate_terminal_surface();
    }

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

    pub(crate) fn take_terminal_refresh_request(&mut self) -> bool {
        let Some(session) = self.active_session() else {
            return false;
        };

        let session_perf = session.perf_snapshot();
        self.perf_overlay.active_session_perf_snapshot = Some(session_perf.clone());
        if !session_perf.dirty_since_last_ui_frame {
            return false;
        }

        self.perf_overlay.mark_terminal_refresh_request(
            Instant::now(),
            RedrawReason::TerminalUpdate,
            Some(session_perf),
        );
        true
    }

    pub(crate) fn invalidate_terminal_surface(&mut self) {
        self.terminal_surface.snapshot_seq = 0;
        self.terminal_surface.geometry = None;
    }

    pub(crate) fn sync_terminal_surface(&mut self, window: &mut Window) {
        let Some(session) = self.active_session() else {
            self.terminal_surface.rows.clear();
            self.terminal_surface.metrics = TerminalRendererMetrics::default();
            self.terminal_surface.active_session_id = 0;
            self.terminal_surface.geometry = None;
            return;
        };

        let metrics = self.terminal_metrics(window);
        let geometry = self
            .last_applied_geometry
            .unwrap_or_else(TerminalGeometry::default);
        let snapshot_seq = self
            .perf_overlay
            .active_session_perf_snapshot
            .as_ref()
            .map(|snapshot| snapshot.terminal.snapshot_seq)
            .unwrap_or(0);
        let needs_rebuild = self.terminal_surface.active_session_id != session.id()
            || self.terminal_surface.snapshot_seq != snapshot_seq
            || self.terminal_surface.geometry != Some(geometry)
            || self.terminal_surface.theme_id != self.active_theme
            || self.terminal_surface.rows.is_empty();

        if !needs_rebuild {
            return;
        }

        let snapshot = session.snapshot();
        let font_family = self.config.terminal.font_family.clone();
        let (rows, metrics_report) = build_terminal_surface_rows(
            &snapshot.rows,
            geometry,
            metrics,
            self.active_theme,
            &self.theme(),
            font_family,
            &mut self.terminal_surface.shape_cache,
            window,
        );

        self.terminal_surface.rows = rows;
        self.terminal_surface.metrics = metrics_report;
        self.terminal_surface.active_session_id = session.id();
        self.terminal_surface.snapshot_seq = snapshot_seq;
        self.terminal_surface.geometry = Some(geometry);
        self.terminal_surface.theme_id = self.active_theme;
    }

    pub(crate) fn toggle_perf_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let next_mode = self.perf_overlay.mode.next();
        if matches!(self.perf_overlay.mode, UiPerfMode::Off) && next_mode.is_enabled() {
            self.perf_overlay.reset_sampling_window();
        }
        self.perf_overlay.mode = next_mode;
        self.perf_overlay
            .mark_ui_refresh_request(Instant::now(), RedrawReason::UiRefresh);
        window.refresh();
        cx.notify();
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
                            self.refresh_saved_hosts();
                            self.refresh_vault_cache();
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
                                self.refresh_saved_hosts();
                                self.refresh_vault_cache();
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
                            self.refresh_saved_hosts();
                            self.refresh_vault_cache();
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
                                        self.refresh_saved_hosts();
                                        self.refresh_vault_cache();
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
        self.refresh_managed_vaults();
        self.saved_hosts.clear();
        self.cached_credentials.clear();
        self.cached_keys.clear();
        self.selected_host_id = None;
        self.secure.host_draft = None;
        self.secure.credential_draft = None;
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
            &self.cached_credentials,
            &self.cached_keys,
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
            PaletteAction::SwitchSession(id) => self.select_session(id, cx),
            PaletteAction::CloseActiveSession => {
                let id = self.active_session_id;
                self.close_session(id, cx);
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
            PaletteAction::ConnectSavedHost { vault_id, host_id } => {
                self.connect_saved_host(&vault_id, &host_id, window, cx);
                return;
            }
            PaletteAction::OpenSftpBrowser(session_id) => {
                self.open_sftp_browser(session_id, cx);
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

        if let Some(bytes) = encode_keystroke(event)
            && let Some(session) = self.active_session()
        {
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
            "backspace" => {
                if matches!(
                    self.vault_modal.mode,
                    UnlockMode::Create | UnlockMode::Rename
                ) && self.vault_modal.selected_field == 0
                {
                    self.vault_modal.vault_name.pop();
                } else if self.vault_modal.selected_field == 0 {
                    self.vault_modal.passphrase.pop();
                } else if matches!(self.vault_modal.mode, UnlockMode::Create)
                    && self.vault_modal.selected_field == 1
                {
                    self.vault_modal.passphrase.pop();
                } else if field_count > 1 {
                    self.vault_modal.confirm_passphrase.pop();
                }
            }
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
                        if matches!(
                            self.vault_modal.mode,
                            UnlockMode::Create | UnlockMode::Rename
                        ) && self.vault_modal.selected_field == 0
                        {
                            self.vault_modal.vault_name.push_str(ch);
                        } else if self.vault_modal.selected_field == 0 {
                            self.vault_modal.passphrase.push_str(ch);
                        } else if matches!(self.vault_modal.mode, UnlockMode::Create)
                            && self.vault_modal.selected_field == 1
                        {
                            self.vault_modal.passphrase.push_str(ch);
                        } else if field_count > 1 {
                            self.vault_modal.confirm_passphrase.push_str(ch);
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
            SecureSection::Keys => SecureInputTarget::KeySearch,
        };
    }

    fn backspace_secure_input(&mut self) {
        match self.secure.input_target {
            SecureInputTarget::HostSearch => {
                self.secure.host_search.pop();
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
        }
    }

    fn push_secure_input(&mut self, text: &str) {
        match self.secure.input_target {
            SecureInputTarget::HostSearch => self.secure.host_search.push_str(text),
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
                self.refresh_saved_hosts();
                if self.secure.host_draft.as_ref().is_some_and(|draft| {
                    draft.host_id.as_deref() == Some(host_id)
                        && draft.vault_id.as_deref() == Some(vault_id)
                }) {
                    self.secure.host_draft = None;
                }
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
        _ => key_char.map(|text| text.as_bytes().to_vec()),
    }
}
