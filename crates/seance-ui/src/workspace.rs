// Owns non-render workspace coordination, input handling, config/update snapshots, and terminal state.

use std::{sync::Arc, time::Instant};

use gpui::{App, Context, FocusHandle, Focusable, KeyDownEvent, Window, font, px};
use seance_config::AppConfig;
use seance_core::UpdateState;
use seance_terminal::TerminalGeometry;
use seance_vault::SecretString;
use tracing::trace;

use crate::{
    app::{InitialWorkspaceAction, refresh_app_menus},
    forms::{CredentialEditorState, CredentialField, HostField, SettingsSection, UnlockMode},
    model::{SeanceWorkspace, TerminalMetrics, TerminalRendererMetrics},
    palette::{PaletteAction, build_items},
    perf::{RedrawReason, UiPerfMode},
    surface::ShapeCache,
    terminal_paint::build_terminal_surface_rows,
    theme::Theme,
    ui_components::{compute_terminal_geometry, theme_id_from_config, update_status_label},
};

impl SeanceWorkspace {
    pub(crate) fn apply_initial_action(
        &mut self,
        action: InitialWorkspaceAction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match action {
            InitialWorkspaceAction::ConnectHost(host_id) => {
                self.selected_host_id = Some(host_id.clone());
                self.connect_saved_host(&host_id, window, cx);
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
        let geometry = compute_terminal_geometry(window.viewport_size(), metrics, self.sidebar_width)
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

    pub(crate) fn submit_unlock_form(&mut self, cx: &mut Context<Self>) {
        match self.unlock_form.mode {
            UnlockMode::Create => {
                if self.unlock_form.passphrase.trim().is_empty() {
                    self.unlock_form.message =
                        Some("Choose a non-empty recovery passphrase.".into());
                } else if self.unlock_form.passphrase != self.unlock_form.confirm_passphrase {
                    self.unlock_form.message = Some("Passphrases do not match yet.".into());
                } else {
                    let passphrase = SecretString::from(self.unlock_form.passphrase.to_string());
                    let result = self.backend.create_vault(&passphrase, "This Device");
                    self.unlock_form.passphrase.clear();
                    self.unlock_form.confirm_passphrase.clear();
                    match result {
                        Ok(()) => {
                            let vault_status = self.backend.vault_status();
                            self.unlock_form.completed = true;
                            self.show_toast(
                                if vault_status.device_unlock_message.is_some() {
                                    "Encrypted vault created. Touch ID is not available for this build yet."
                                } else {
                                    "Encrypted vault created. Device unlock is now enrolled."
                                },
                            );
                            self.refresh_saved_hosts();
                            self.refresh_vault_cache();
                        }
                        Err(err) => {
                            self.unlock_form.message = Some(err.to_string());
                        }
                    }
                }
            }
            UnlockMode::Unlock => {
                let passphrase = SecretString::from(self.unlock_form.passphrase.to_string());
                let result = self.backend.unlock_vault(&passphrase, "This Device");
                self.unlock_form.passphrase.clear();
                self.unlock_form.confirm_passphrase.clear();
                match result {
                    Ok(()) => {
                        let vault_status = self.backend.vault_status();
                        self.unlock_form.completed = true;
                        self.show_toast(
                            if vault_status.device_unlock_message.is_some() {
                                "Vault unlocked from the recovery passphrase. Touch ID is not available for this build yet."
                            } else {
                                "Vault unlocked from the recovery passphrase."
                            },
                        );
                        self.refresh_saved_hosts();
                        self.refresh_vault_cache();
                    }
                    Err(err) => {
                        self.unlock_form.message = Some(err.to_string());
                    }
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        refresh_app_menus(cx);
        cx.notify();
    }

    pub(crate) fn lock_vault(&mut self, cx: &mut Context<Self>) {
        self.backend.lock_vault();
        self.saved_hosts.clear();
        self.cached_credentials.clear();
        self.cached_keys.clear();
        self.selected_host_id = None;
        self.host_editor = None;
        self.credential_editor = None;
        self.settings_panel.open = false;
        self.unlock_form.reset_for_unlock();
        self.unlock_form.message =
            Some("Vault locked. Decrypted records were cleared from memory.".into());
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
        }
        self.perf_overlay.mark_input(RedrawReason::Palette);
        cx.notify();
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
                self.unlock_form.reset_for_unlock();
                self.unlock_form.message =
                    Some("Enter the recovery passphrase to unlock the vault.".into());
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
                self.credential_editor = Some(CredentialEditorState::blank());
            }
            PaletteAction::EditPasswordCredential(id) => {
                self.begin_edit_credential(&id, cx);
                return;
            }
            PaletteAction::DeletePasswordCredential(id) => {
                self.delete_credential(&id, cx);
                return;
            }
            PaletteAction::ImportPrivateKey => {
                self.show_toast(
                    "Private key import backend is ready; UI import form is still pending.",
                );
            }
            PaletteAction::GenerateEd25519Key => {
                match self
                    .backend
                    .generate_ed25519_key(format!("ed25519-{}", crate::now_ui_suffix()))
                {
                    Ok(summary) => {
                        self.show_toast(format!("Generated vault-backed key '{}'.", summary.label));
                        self.refresh_vault_cache();
                    }
                    Err(err) => self.show_toast(err.to_string()),
                }
            }
            PaletteAction::GenerateRsaKey => {
                match self
                    .backend
                    .generate_rsa_key(format!("rsa-{}", crate::now_ui_suffix()))
                {
                    Ok(summary) => {
                        self.show_toast(format!("Generated vault-backed key '{}'.", summary.label));
                        self.refresh_vault_cache();
                    }
                    Err(err) => self.show_toast(err.to_string()),
                }
            }
            PaletteAction::DeletePrivateKey(id) => {
                self.delete_private_key(&id, cx);
                return;
            }
            PaletteAction::EditSavedHost(id) => {
                self.begin_edit_host(&id, cx);
                return;
            }
            PaletteAction::DeleteSavedHost(id) => {
                self.delete_saved_host(&id, cx);
                return;
            }
            PaletteAction::ConnectSavedHost(id) => {
                self.connect_saved_host(&id, window, cx);
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
        cx.notify();
    }

    pub(crate) fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();

        if self.unlock_form.is_visible() {
            self.handle_unlock_key(event, cx);
            return;
        }
        if self.credential_editor.is_some() {
            self.handle_credential_editor_key(event, cx);
            return;
        }
        if self.host_editor.is_some() {
            self.handle_host_editor_key(event, cx);
            return;
        }

        if self.palette_open {
            self.handle_palette_key(event, window, cx);
            return;
        }

        if self.sftp_browser.is_some() {
            self.handle_sftp_key(event, window, cx);
            return;
        }

        if self.is_settings_panel_open() && key == "escape" {
            self.close_settings_panel(cx);
            self.perf_overlay.mark_input(RedrawReason::Input);
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
                self.palette_selected = self.palette_selected.saturating_sub(1);
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "down" => {
                let session_labels = self.palette_session_labels();
                let remote_ids = self.remote_session_ids();
                let sessions = self.sessions();
                let count = build_items(
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
                )
                .len();
                if self.palette_selected + 1 < count {
                    self.palette_selected += 1;
                }
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "enter" => {
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
                if let Some(item) = items.get(self.palette_selected) {
                    let action = item.action.clone();
                    self.execute_palette_action(action, window, cx);
                }
            }
            "backspace" => {
                self.palette_query.pop();
                self.palette_selected = 0;
                self.perf_overlay.mark_input(RedrawReason::Palette);
                cx.notify();
            }
            "tab" | "left" | "right" | "home" | "end" | "pageup" | "pagedown" => {}
            _ => {
                if let Some(ch) = key_char {
                    let modifiers = event.keystroke.modifiers;
                    if !modifiers.platform && !modifiers.control && !modifiers.function {
                        self.palette_query.push_str(ch);
                        self.palette_selected = 0;
                        self.perf_overlay.mark_input(RedrawReason::Palette);
                        cx.notify();
                    }
                }
            }
        }
    }

    fn handle_unlock_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let key_char = event.keystroke.key_char.as_deref();

        match key {
            "tab" | "down" => {
                let count = if matches!(self.unlock_form.mode, UnlockMode::Create) {
                    2
                } else {
                    1
                };
                self.unlock_form.selected_field = (self.unlock_form.selected_field + 1) % count;
            }
            "up" => {
                let count = if matches!(self.unlock_form.mode, UnlockMode::Create) {
                    2
                } else {
                    1
                };
                self.unlock_form.selected_field =
                    (self.unlock_form.selected_field + count - 1) % count;
            }
            "backspace" => {
                if self.unlock_form.selected_field == 0 {
                    self.unlock_form.passphrase.pop();
                } else {
                    self.unlock_form.confirm_passphrase.pop();
                }
            }
            "enter" => {
                self.submit_unlock_form(cx);
                return;
            }
            "escape" => {}
            _ => {
                if let Some(ch) = key_char {
                    let modifiers = event.keystroke.modifiers;
                    if !modifiers.platform && !modifiers.control && !modifiers.function {
                        if self.unlock_form.selected_field == 0 {
                            self.unlock_form.passphrase.push_str(ch);
                        } else {
                            self.unlock_form.confirm_passphrase.push_str(ch);
                        }
                    }
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn handle_host_editor_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let key_char = event.keystroke.key_char.as_deref();
        let modifiers = event.keystroke.modifiers;

        if modifiers.platform && key == "s" {
            self.save_host_editor(cx);
            return;
        }

        let Some(editor) = self.host_editor.as_mut() else {
            return;
        };

        let in_auth = matches!(editor.field(), HostField::Auth);

        match key {
            "escape" => {
                self.host_editor = None;
            }
            "tab" => {
                if modifiers.shift {
                    editor.selected_field =
                        (editor.selected_field + HostField::ALL.len() - 1) % HostField::ALL.len();
                } else {
                    editor.selected_field = (editor.selected_field + 1) % HostField::ALL.len();
                }
                editor.auth_cursor = 0;
            }
            "down" => {
                if in_auth {
                    let total = self.cached_credentials.len() + self.cached_keys.len();
                    if total > 0 {
                        editor.auth_cursor = (editor.auth_cursor + 1).min(total - 1);
                    }
                } else {
                    editor.selected_field = (editor.selected_field + 1) % HostField::ALL.len();
                    editor.auth_cursor = 0;
                }
            }
            "up" => {
                if in_auth {
                    editor.auth_cursor = editor.auth_cursor.saturating_sub(1);
                } else {
                    editor.selected_field =
                        (editor.selected_field + HostField::ALL.len() - 1) % HostField::ALL.len();
                    editor.auth_cursor = 0;
                }
            }
            "backspace" => match editor.field() {
                HostField::Label => {
                    editor.label.pop();
                }
                HostField::Hostname => {
                    editor.hostname.pop();
                }
                HostField::Username => {
                    editor.username.pop();
                }
                HostField::Port => {
                    editor.port.pop();
                }
                HostField::Notes => {
                    editor.notes.pop();
                }
                HostField::Auth => {}
            },
            "enter" | " " if in_auth => {
                self.toggle_host_auth_at_cursor();
            }
            "enter" => {
                editor.selected_field = (editor.selected_field + 1) % HostField::ALL.len();
            }
            _ => {
                if let Some(ch) = key_char {
                    if !modifiers.platform && !modifiers.control && !modifiers.function && !in_auth
                    {
                        match editor.field() {
                            HostField::Label => editor.label.push_str(ch),
                            HostField::Hostname => editor.hostname.push_str(ch),
                            HostField::Username => editor.username.push_str(ch),
                            HostField::Port => {
                                if ch.chars().all(|value| value.is_ascii_digit()) {
                                    editor.port.push_str(ch);
                                }
                            }
                            HostField::Notes => editor.notes.push_str(ch),
                            HostField::Auth => {}
                        }
                    }
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn handle_credential_editor_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(editor) = self.credential_editor.as_mut() else {
            return;
        };
        let key = event.keystroke.key.as_str();
        let key_char = event.keystroke.key_char.as_deref();
        let modifiers = event.keystroke.modifiers;

        match key {
            "escape" => {
                self.credential_editor = None;
            }
            "tab" | "down" => {
                editor.selected_field = (editor.selected_field + 1) % CredentialField::ALL.len();
            }
            "up" => {
                editor.selected_field = (editor.selected_field + CredentialField::ALL.len() - 1)
                    % CredentialField::ALL.len();
            }
            "backspace" => match editor.field() {
                CredentialField::Label => {
                    editor.label.pop();
                }
                CredentialField::UsernameHint => {
                    editor.username_hint.pop();
                }
                CredentialField::Secret => {
                    editor.secret.pop();
                }
            },
            "enter" => {
                if matches!(editor.field(), CredentialField::Secret) {
                    self.save_credential_editor(cx);
                    return;
                }
                editor.selected_field = (editor.selected_field + 1) % CredentialField::ALL.len();
            }
            _ => {
                if let Some(ch) = key_char {
                    if !modifiers.platform && !modifiers.control && !modifiers.function {
                        match editor.field() {
                            CredentialField::Label => editor.label.push_str(ch),
                            CredentialField::UsernameHint => editor.username_hint.push_str(ch),
                            CredentialField::Secret => editor.secret.push_str(ch),
                        }
                    }
                }
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
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
