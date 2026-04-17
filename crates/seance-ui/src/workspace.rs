// Owns non-render workspace coordination, input handling, config/update snapshots, and terminal state.

use std::{sync::Arc, time::Duration};

use gpui::{
    App, ClipboardItem, Context, FocusHandle, Focusable, KeyDownEvent, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ScrollDelta, ScrollWheelEvent, Window, point, px,
};
use seance_config::AppConfig;
use seance_core::{UpdateState, VaultUiSnapshot};
use seance_terminal::{
    TerminalInputModifiers, TerminalMouseButton, TerminalMouseEvent, TerminalMouseEventKind,
    TerminalScreenKind, TerminalScrollCommand, TerminalViewportSnapshot,
};
use seance_vault::{SecretString, UnlockMethod};

use crate::{
    TERMINAL_SCROLLBAR_GUTTER_WIDTH_PX, TerminalScrollbarDragState, TerminalScrollbarHit,
    TerminalScrollbarLayout,
    app::{InitialWorkspaceAction, refresh_app_menus},
    forms::{
        CredentialDraftField, HostDraftField, SecureInputTarget, SecureSection, SettingsSection,
        TunnelDraftField, UnlockMode, VaultModalOrigin, WorkspaceSurface,
    },
    model::{
        SeanceWorkspace, TerminalHoveredLink, TerminalSelectionPoint, sidebar_occupied_width_px,
    },
    palette::{
        PageDirection, PaletteAction, PaletteViewModel, build_items, flatten_items,
        page_target_index,
    },
    perf::RedrawReason,
    surface::ShapeCache,
    terminal_links::{terminal_link_at_column, terminal_links_for_row},
    theme::Theme,
    ui_components::{TERMINAL_PANE_PADDING_PX, theme_id_from_config, update_status_label},
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
    fn set_terminal_hovered_link(&mut self, next: Option<TerminalHoveredLink>) -> bool {
        if self.terminal_hovered_link == next {
            return false;
        }

        self.terminal_hovered_link = next;
        true
    }

    fn clear_terminal_hovered_link(&mut self) -> bool {
        self.set_terminal_hovered_link(None)
    }

    fn update_terminal_hovered_link(
        &mut self,
        position: gpui::Point<gpui::Pixels>,
        modifiers: gpui::Modifiers,
    ) -> bool {
        let next = self.active_session().and_then(|session| {
            let summary = session.summary();
            let point = self.terminal_selection_point(position)?;
            let viewport = session.viewport_snapshot();
            terminal_hovered_link_at_position(
                &viewport,
                point,
                summary.active_screen,
                summary.mouse_tracking,
                modifiers,
            )
        });

        self.set_terminal_hovered_link(next)
    }

    pub(crate) fn reconcile_terminal_hovered_link(
        &mut self,
        viewport: &TerminalViewportSnapshot,
        active_screen: TerminalScreenKind,
        mouse_tracking: bool,
        modifiers: gpui::Modifiers,
    ) {
        let Some(hovered_link) = self.terminal_hovered_link.as_mut() else {
            return;
        };

        if !matches!(active_screen, TerminalScreenKind::Primary) || mouse_tracking {
            self.terminal_hovered_link = None;
            return;
        }

        let Some(row) = viewport.rows.get(hovered_link.row) else {
            self.terminal_hovered_link = None;
            return;
        };
        let Some(row_revision) = viewport.row_revisions.get(hovered_link.row).copied() else {
            self.terminal_hovered_link = None;
            return;
        };
        let visible_cols = viewport.cols as usize;
        let link_still_present = terminal_links_for_row(row, visible_cols)
            .into_iter()
            .any(|link| link.col_range == hovered_link.col_range && link.url == hovered_link.url);
        if row_revision != hovered_link.row_revision || !link_still_present {
            self.terminal_hovered_link = None;
            return;
        }

        hovered_link.modifier_active = terminal_link_open_modifier(modifiers);
    }

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
        let line_height = self
            .terminal_metrics
            .map(|metrics| metrics.line_height_px)
            .unwrap_or_else(|| self.terminal_line_height_px())
            .max(1.0);
        let delta_rows_f = match event.delta {
            ScrollDelta::Pixels(delta) => -(f32::from(delta.y) / line_height),
            ScrollDelta::Lines(delta) => -delta.y,
        };
        if delta_rows_f.abs() < f32::EPSILON {
            return;
        }
        if self.terminal_scroll_remainder_rows.signum() != 0.0
            && delta_rows_f.signum() != 0.0
            && self.terminal_scroll_remainder_rows.signum() != delta_rows_f.signum()
        {
            self.terminal_scroll_remainder_rows = 0.0;
        }
        let pending_rows = self.terminal_scroll_remainder_rows + delta_rows_f;
        let delta_rows = pending_rows.trunc() as isize;
        self.terminal_scroll_remainder_rows = pending_rows - delta_rows as f32;

        if summary.mouse_tracking {
            if delta_rows == 0 {
                return;
            }
            let button = if delta_rows > 0 {
                TerminalMouseButton::WheelUp
            } else {
                TerminalMouseButton::WheelDown
            };
            if let Some(mouse_event) = self.terminal_mouse_event(
                event.position,
                TerminalMouseEventKind::Press,
                Some(button),
                event.modifiers,
            ) {
                for _ in 0..delta_rows.unsigned_abs() {
                    let _ = session.send_mouse(mouse_event.clone());
                }
                self.perf_overlay.mark_input(RedrawReason::TerminalUpdate);
                cx.notify();
            }
            return;
        }

        if matches!(summary.active_screen, TerminalScreenKind::Alternate) {
            self.terminal_scroll_remainder_rows = 0.0;
            return;
        }

        if delta_rows == 0 {
            return;
        }

        let _ = session.scroll_viewport(TerminalScrollCommand::DeltaRows(delta_rows));
        self.perf_overlay.mark_input(RedrawReason::TerminalUpdate);
        cx.notify();
    }

    pub(crate) fn handle_terminal_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);

        let Some(session) = self.active_session() else {
            return;
        };
        let summary = session.summary();
        let scrollbar_interactive =
            terminal_scrollbar_is_interactive(summary.active_screen, summary.mouse_tracking);
        if event.button == MouseButton::Left
            && scrollbar_interactive
            && let Some((layout, local_x, local_y)) =
                self.terminal_scrollbar_local_position(event.position)
            && let Some(outcome) = terminal_scrollbar_mouse_down_outcome(layout, local_x, local_y)
        {
            self.terminal_scrollbar_hovered = true;
            self.terminal_scrollbar_drag = Some(outcome.drag_state);
            if let Some(command) = outcome.command {
                let _ = session.scroll_viewport(command);
                self.perf_overlay.mark_input(RedrawReason::TerminalUpdate);
            } else {
                self.perf_overlay.mark_input(RedrawReason::Input);
            }
            cx.notify();
            return;
        }

        if summary.mouse_tracking {
            self.clear_terminal_hovered_link();
            self.terminal_scrollbar_hovered = false;
            self.terminal_scrollbar_drag = None;
            if let Some(mouse_event) = self.terminal_mouse_event(
                event.position,
                TerminalMouseEventKind::Press,
                terminal_mouse_button(event.button),
                event.modifiers,
            ) {
                let _ = session.send_mouse(mouse_event);
            }
            self.clear_terminal_selection();
        } else if self.try_open_terminal_link(event, summary.active_screen) {
            self.terminal_drag_anchor = None;
            self.clear_terminal_hovered_link();
            self.perf_overlay.mark_input(RedrawReason::Input);
            cx.notify();
            return;
        } else if event.button == MouseButton::Left
            && let Some(point) = self.terminal_selection_point(event.position)
        {
            self.clear_terminal_hovered_link();
            self.terminal_selection = Some(crate::model::TerminalSelection {
                anchor: point,
                focus: point,
            });
            self.terminal_drag_anchor = Some(point);
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    pub(crate) fn handle_terminal_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.active_session() else {
            if self.clear_terminal_hovered_link() {
                self.perf_overlay.mark_input(RedrawReason::Input);
                cx.notify();
            }
            return;
        };
        let summary = session.summary();
        let scrollbar_interactive =
            terminal_scrollbar_is_interactive(summary.active_screen, summary.mouse_tracking);
        if let Some(drag_state) = self.terminal_scrollbar_drag {
            let mut state_changed = self.clear_terminal_hovered_link();
            if scrollbar_interactive && let Some(local_y) = self.terminal_local_y(event.position) {
                let _ =
                    session.scroll_viewport(terminal_scrollbar_drag_command(drag_state, local_y));
                self.terminal_scrollbar_hovered = true;
                self.perf_overlay.mark_input(RedrawReason::TerminalUpdate);
                cx.notify();
            } else {
                self.terminal_scrollbar_drag = None;
                if self.terminal_scrollbar_hovered {
                    self.terminal_scrollbar_hovered = false;
                    state_changed = true;
                }
                if state_changed {
                    self.perf_overlay.mark_input(RedrawReason::Input);
                    cx.notify();
                }
            }
            return;
        }

        if summary.mouse_tracking {
            let mut state_changed = self.clear_terminal_hovered_link();
            if self.terminal_scrollbar_hovered {
                self.terminal_scrollbar_hovered = false;
                state_changed = true;
            }
            if let Some(mouse_event) = self.terminal_mouse_event(
                event.position,
                TerminalMouseEventKind::Move,
                event.pressed_button.and_then(terminal_mouse_button),
                event.modifiers,
            ) {
                let _ = session.send_mouse(mouse_event);
                self.perf_overlay.mark_input(RedrawReason::TerminalUpdate);
                cx.notify();
            } else if state_changed {
                self.perf_overlay.mark_input(RedrawReason::Input);
                cx.notify();
            }
            return;
        }

        let scrollbar_hovered = scrollbar_interactive
            && self
                .terminal_scrollbar_local_position(event.position)
                .is_some_and(|(layout, local_x, local_y)| {
                    layout.hit_test(local_x, local_y).is_some()
                });
        let mut state_changed = false;
        if scrollbar_hovered != self.terminal_scrollbar_hovered {
            self.terminal_scrollbar_hovered = scrollbar_hovered;
            state_changed = true;
        }

        if event.dragging() {
            state_changed |= self.clear_terminal_hovered_link();
        } else {
            state_changed |= self.update_terminal_hovered_link(event.position, event.modifiers);
        }

        if state_changed {
            self.perf_overlay.mark_input(RedrawReason::Input);
            cx.notify();
        }

        if !event.dragging() {
            return;
        }

        let Some(anchor) = self.terminal_drag_anchor else {
            return;
        };
        let Some(focus) = self.terminal_selection_point(event.position) else {
            return;
        };
        self.terminal_selection = Some(crate::model::TerminalSelection { anchor, focus });
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    pub(crate) fn handle_terminal_mouse_up(
        &mut self,
        event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.active_session() else {
            self.terminal_drag_anchor = None;
            self.terminal_hovered_link = None;
            self.terminal_scrollbar_hovered = false;
            self.terminal_scrollbar_drag = None;
            return;
        };

        let summary = session.summary();
        let scrollbar_interactive =
            terminal_scrollbar_is_interactive(summary.active_screen, summary.mouse_tracking);
        if self.terminal_scrollbar_drag.take().is_some() {
            self.terminal_drag_anchor = None;
            self.terminal_scrollbar_hovered = scrollbar_interactive
                && self
                    .terminal_scrollbar_local_position(event.position)
                    .is_some_and(|(layout, local_x, local_y)| {
                        layout.hit_test(local_x, local_y).is_some()
                    });
            self.perf_overlay.mark_input(RedrawReason::Input);
            cx.notify();
            return;
        }

        if summary.mouse_tracking {
            if let Some(mouse_event) = self.terminal_mouse_event(
                event.position,
                TerminalMouseEventKind::Release,
                terminal_mouse_button(event.button),
                event.modifiers,
            ) {
                let _ = session.send_mouse(mouse_event);
            }
        }

        self.terminal_drag_anchor = None;
        self.perf_overlay.mark_input(RedrawReason::Input);
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

    pub(crate) fn sidebar_occupied_width_px(&self) -> f32 {
        sidebar_occupied_width_px(self.sidebar_width)
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
        self.sync_vault_modal_text_input();
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
        self.close_palette(cx);
        self.invalidate_terminal_surface();
        self.perf_overlay.mark_input(RedrawReason::Input);
        refresh_app_menus(cx);
        cx.notify();
    }

    pub(crate) fn toggle_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette_open {
            self.close_palette(cx);
        } else {
            self.palette_open = true;
            self.palette_query.clear();
            self.palette_text_input = crate::TextEditState::default();
            self.palette_selected = 0;
            self.reset_palette_scroll_to_top();
            self.perf_overlay.mark_input(RedrawReason::Palette);
            cx.notify();
        }
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

    pub(crate) fn close_palette(&mut self, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }

        self.palette_open = false;
        self.palette_query.clear();
        self.palette_selected = 0;
        self.palette_text_input = crate::TextEditState::default();
        self.reset_palette_scroll_to_top();
        self.perf_overlay.mark_input(RedrawReason::Palette);
        cx.notify();
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
            }
            PaletteAction::AddSavedHost => {
                self.begin_add_host(cx);
            }
            PaletteAction::OpenVaultPanel => {
                self.open_vault_panel(cx);
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
            }
            PaletteAction::DeletePasswordCredential {
                vault_id,
                credential_id,
            } => {
                self.attempt_delete_credential(&item_scope_key(&vault_id, &credential_id), cx);
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
            }
            PaletteAction::EditSavedHost { vault_id, host_id } => {
                self.begin_edit_host(&host_scope_key(&vault_id, &host_id), cx);
            }
            PaletteAction::DeleteSavedHost { vault_id, host_id } => {
                self.delete_saved_host(&host_scope_key(&vault_id, &host_id), cx);
            }
            PaletteAction::CancelSavedHostConnect { attempt_id } => {
                self.cancel_connect_attempt(attempt_id, cx);
            }
            PaletteAction::ConnectSavedHost { vault_id, host_id } => {
                self.start_connect_attempt(&vault_id, &host_id, window, cx);
            }
            PaletteAction::OpenSftpBrowser(session_id) => {
                self.open_sftp_browser(session_id, cx);
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
            }
            PaletteAction::OpenHostTunnelSettings { vault_id, host_id } => {
                self.activate_host_draft(Some(&host_scope_key(&vault_id, &host_id)), cx);
            }
            PaletteAction::StartTunnel {
                vault_id,
                port_forward_id,
            } => {
                self.start_saved_tunnel(&item_scope_key(&vault_id, &port_forward_id), cx);
            }
            PaletteAction::StopTunnel { tunnel_scope_key } => {
                self.stop_saved_tunnel(&tunnel_scope_key, cx);
            }
            PaletteAction::OpenPreferences => {
                self.open_settings_panel(SettingsSection::General, cx);
            }
        }
        self.close_palette(cx);
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

        if self.handle_terminal_input_key(event, cx) {
            return;
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
                self.close_palette(cx);
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
            "tab" => {}
            _ => {
                if self.handle_palette_text_input(event, cx) {
                    if text_input_mutated_key(event, key_char) {
                        self.palette_selected = 0;
                        self.reset_palette_scroll_to_top();
                    }
                    self.perf_overlay.mark_input(RedrawReason::Palette);
                    cx.notify();
                }
            }
        }
    }

    fn handle_vault_modal_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let field_count = self.vault_modal.passphrase_field_count();

        match key {
            "tab" | "down" => {
                if field_count > 0 {
                    self.focus_vault_modal_field(
                        (self.vault_modal.selected_field + 1) % field_count,
                    );
                }
            }
            "up" => {
                if field_count > 0 {
                    self.focus_vault_modal_field(
                        (self.vault_modal.selected_field + field_count - 1) % field_count,
                    );
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
                let _ = self.handle_vault_modal_text_input(event, cx);
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn handle_secure_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let modifiers = event.keystroke.modifiers;

        if text_primary_modifier(modifiers) && key == "s" {
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
            _ => {
                let _ = self.handle_secure_text_input(event, cx);
            }
        }

        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn cycle_secure_focus(&mut self, backward: bool) {
        let next_target = match self.secure.section {
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
        self.focus_secure_input_target(next_target);
    }

    pub(crate) fn focus_vault_modal_field(&mut self, field: usize) {
        self.vault_modal.selected_field = field;
        self.vault_modal_text_field = Some(field);
        self.vault_modal_text_input = self
            .vault_modal_field_value(field)
            .map(|value| crate::TextEditState::with_text(&value))
            .unwrap_or_default();
    }

    pub(crate) fn sync_vault_modal_text_input(&mut self) {
        let field_count = self.vault_modal.passphrase_field_count();
        if field_count == 0 {
            self.vault_modal_text_field = None;
            self.vault_modal_text_input = crate::TextEditState::default();
            return;
        }

        let field = self
            .vault_modal
            .selected_field
            .min(field_count.saturating_sub(1));
        let value = self.vault_modal_field_value(field).unwrap_or_default();
        if self.vault_modal_text_field == Some(field) {
            self.vault_modal_text_input.sync(&value);
        } else {
            self.vault_modal.selected_field = field;
            self.vault_modal_text_field = Some(field);
            self.vault_modal_text_input = crate::TextEditState::with_text(&value);
        }
    }

    fn vault_modal_field_value(&self, field: usize) -> Option<String> {
        match self.vault_modal.mode {
            UnlockMode::Create => match field {
                0 => Some(self.vault_modal.vault_name.to_string()),
                1 => Some(self.vault_modal.passphrase.to_string()),
                2 => Some(self.vault_modal.confirm_passphrase.to_string()),
                _ => None,
            },
            UnlockMode::Rename => (field == 0).then(|| self.vault_modal.vault_name.to_string()),
            UnlockMode::Unlock => (self.vault_modal.unlock_method == UnlockMethod::Passphrase
                && field == 0)
                .then(|| self.vault_modal.passphrase.to_string()),
        }
    }

    fn handle_vault_modal_text_input(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        self.sync_vault_modal_text_input();
        let field = match self.vault_modal_text_field {
            Some(field) => field,
            None => return false,
        };
        let paste = text_input_paste_text(event, cx);
        let outcome = match self.vault_modal.mode {
            UnlockMode::Create => match field {
                0 => apply_text_input_event(
                    &mut self.vault_modal_text_input,
                    &mut self.vault_modal.vault_name,
                    event,
                    paste.as_deref(),
                ),
                1 => apply_text_input_event(
                    &mut self.vault_modal_text_input,
                    &mut self.vault_modal.passphrase,
                    event,
                    paste.as_deref(),
                ),
                2 => apply_text_input_event(
                    &mut self.vault_modal_text_input,
                    &mut self.vault_modal.confirm_passphrase,
                    event,
                    paste.as_deref(),
                ),
                _ => TextInputOutcome::ignored(),
            },
            UnlockMode::Rename => {
                if field == 0 {
                    apply_text_input_event(
                        &mut self.vault_modal_text_input,
                        &mut self.vault_modal.vault_name,
                        event,
                        paste.as_deref(),
                    )
                } else {
                    TextInputOutcome::ignored()
                }
            }
            UnlockMode::Unlock => {
                if field == 0 && self.vault_modal.unlock_method == UnlockMethod::Passphrase {
                    apply_text_input_event(
                        &mut self.vault_modal_text_input,
                        &mut self.vault_modal.passphrase,
                        event,
                        paste.as_deref(),
                    )
                } else {
                    TextInputOutcome::ignored()
                }
            }
        };
        apply_text_input_outcome(&outcome, cx);
        outcome.consumed
    }

    pub(crate) fn focus_secure_input_target(&mut self, target: SecureInputTarget) {
        self.secure.input_target = target;
        match target {
            SecureInputTarget::HostDraft(field) => {
                if let Some(draft) = self.secure.host_draft.as_mut() {
                    draft.selected_field = field;
                }
            }
            SecureInputTarget::TunnelDraft(field) => {
                if let Some(draft) = self.secure.tunnel_draft.as_mut() {
                    draft.selected_field = field;
                }
            }
            SecureInputTarget::CredentialDraft(field) => {
                if let Some(draft) = self.secure.credential_draft.as_mut() {
                    draft.selected_field = field;
                }
            }
            SecureInputTarget::HostSearch
            | SecureInputTarget::TunnelSearch
            | SecureInputTarget::CredentialSearch
            | SecureInputTarget::KeySearch => {}
        }

        self.secure_text_target = Some(target);
        self.secure_text_input = self
            .secure_text_value(target)
            .map(|value| crate::TextEditState::with_text(&value))
            .unwrap_or_default();
    }

    fn sync_secure_text_input(&mut self) {
        let target = self.secure.input_target;
        let value = match self.secure_text_value(target) {
            Some(value) => value,
            None => {
                self.secure_text_target = Some(target);
                self.secure_text_input = crate::TextEditState::default();
                return;
            }
        };

        if self.secure_text_target == Some(target) {
            self.secure_text_input.sync(&value);
        } else {
            self.secure_text_target = Some(target);
            self.secure_text_input = crate::TextEditState::with_text(&value);
        }
    }

    fn secure_text_value(&self, target: SecureInputTarget) -> Option<String> {
        match target {
            SecureInputTarget::HostSearch => Some(self.secure.host_search.clone()),
            SecureInputTarget::TunnelSearch => Some(self.secure.tunnel_search.clone()),
            SecureInputTarget::CredentialSearch => Some(self.secure.credential_search.clone()),
            SecureInputTarget::KeySearch => Some(self.secure.key_search.clone()),
            SecureInputTarget::HostDraft(field) => {
                self.secure.host_draft.as_ref().map(|draft| match field {
                    HostDraftField::Label => draft.label.clone(),
                    HostDraftField::Hostname => draft.hostname.clone(),
                    HostDraftField::Username => draft.username.clone(),
                    HostDraftField::Port => draft.port.clone(),
                    HostDraftField::Notes => draft.notes.clone(),
                })
            }
            SecureInputTarget::CredentialDraft(field) => {
                self.secure
                    .credential_draft
                    .as_ref()
                    .map(|draft| match field {
                        CredentialDraftField::Label => draft.label.clone(),
                        CredentialDraftField::UsernameHint => draft.username_hint.clone(),
                        CredentialDraftField::Secret => draft.secret.clone(),
                    })
            }
            SecureInputTarget::TunnelDraft(field) => {
                self.secure.tunnel_draft.as_ref().map(|draft| match field {
                    TunnelDraftField::Label => draft.label.clone(),
                    TunnelDraftField::Mode => {
                        if draft.mode == seance_vault::PortForwardMode::Local {
                            "Local".into()
                        } else {
                            "Remote".into()
                        }
                    }
                    TunnelDraftField::ListenAddress => draft.listen_address.clone(),
                    TunnelDraftField::ListenPort => draft.listen_port.clone(),
                    TunnelDraftField::TargetAddress => draft.target_address.clone(),
                    TunnelDraftField::TargetPort => draft.target_port.clone(),
                    TunnelDraftField::Notes => draft.notes.clone(),
                })
            }
        }
    }

    fn handle_secure_text_input(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.secure.input_target == SecureInputTarget::TunnelDraft(TunnelDraftField::Mode) {
            if let Some(ch) = event.keystroke.key_char.as_deref()
                && let Some(draft) = self.secure.tunnel_draft.as_mut()
            {
                if ch.eq_ignore_ascii_case("l") {
                    draft.mode = seance_vault::PortForwardMode::Local;
                    draft.dirty = true;
                    return true;
                }
                if ch.eq_ignore_ascii_case("r") {
                    draft.mode = seance_vault::PortForwardMode::Remote;
                    draft.dirty = true;
                    return true;
                }
            }
            return false;
        }

        self.sync_secure_text_input();
        let target = self.secure.input_target;
        let paste =
            text_input_paste_text(event, cx).map(|text| filter_secure_input_text(target, &text));
        let paste = paste.as_deref();

        let outcome = match target {
            SecureInputTarget::HostSearch => apply_text_input_event(
                &mut self.secure_text_input,
                &mut self.secure.host_search,
                event,
                paste,
            ),
            SecureInputTarget::TunnelSearch => apply_text_input_event(
                &mut self.secure_text_input,
                &mut self.secure.tunnel_search,
                event,
                paste,
            ),
            SecureInputTarget::CredentialSearch => apply_text_input_event(
                &mut self.secure_text_input,
                &mut self.secure.credential_search,
                event,
                paste,
            ),
            SecureInputTarget::KeySearch => apply_text_input_event(
                &mut self.secure_text_input,
                &mut self.secure.key_search,
                event,
                paste,
            ),
            SecureInputTarget::HostDraft(field) => {
                let Some(draft) = self.secure.host_draft.as_mut() else {
                    return false;
                };
                let outcome = match field {
                    HostDraftField::Label => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.label,
                        event,
                        paste,
                    ),
                    HostDraftField::Hostname => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.hostname,
                        event,
                        paste,
                    ),
                    HostDraftField::Username => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.username,
                        event,
                        paste,
                    ),
                    HostDraftField::Port => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.port,
                        event,
                        paste,
                    ),
                    HostDraftField::Notes => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.notes,
                        event,
                        paste,
                    ),
                };
                if outcome.changed {
                    draft.dirty = true;
                }
                outcome
            }
            SecureInputTarget::CredentialDraft(field) => {
                let Some(draft) = self.secure.credential_draft.as_mut() else {
                    return false;
                };
                let outcome = match field {
                    CredentialDraftField::Label => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.label,
                        event,
                        paste,
                    ),
                    CredentialDraftField::UsernameHint => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.username_hint,
                        event,
                        paste,
                    ),
                    CredentialDraftField::Secret => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.secret,
                        event,
                        paste,
                    ),
                };
                if outcome.changed {
                    draft.dirty = true;
                }
                outcome
            }
            SecureInputTarget::TunnelDraft(field) => {
                let Some(draft) = self.secure.tunnel_draft.as_mut() else {
                    return false;
                };
                let outcome = match field {
                    TunnelDraftField::Label => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.label,
                        event,
                        paste,
                    ),
                    TunnelDraftField::Mode => TextInputOutcome::ignored(),
                    TunnelDraftField::ListenAddress => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.listen_address,
                        event,
                        paste,
                    ),
                    TunnelDraftField::ListenPort => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.listen_port,
                        event,
                        paste,
                    ),
                    TunnelDraftField::TargetAddress => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.target_address,
                        event,
                        paste,
                    ),
                    TunnelDraftField::TargetPort => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.target_port,
                        event,
                        paste,
                    ),
                    TunnelDraftField::Notes => apply_text_input_event(
                        &mut self.secure_text_input,
                        &mut draft.notes,
                        event,
                        paste,
                    ),
                };
                if outcome.changed {
                    draft.dirty = true;
                }
                outcome
            }
        };

        apply_text_input_outcome(&outcome, cx);
        outcome.consumed
    }

    fn handle_palette_text_input(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        let paste = text_input_paste_text(event, cx);
        let outcome = apply_text_input_event(
            &mut self.palette_text_input,
            &mut self.palette_query,
            event,
            paste.as_deref(),
        );
        apply_text_input_outcome(&outcome, cx);
        outcome.consumed
    }

    fn clear_terminal_selection(&mut self) {
        self.terminal_selection = None;
        self.terminal_drag_anchor = None;
    }

    pub(crate) fn terminal_scrollbar_layout(&self) -> Option<TerminalScrollbarLayout> {
        let geometry = self.terminal_surface.geometry?;
        let scrollbar = self.terminal_surface.scrollbar?;
        TerminalScrollbarLayout::new(scrollbar, geometry)
    }

    fn terminal_local_y(&self, position: gpui::Point<gpui::Pixels>) -> Option<f32> {
        let geometry = self.terminal_surface.geometry?;
        let local_y = f32::from(position.y) - TERMINAL_PANE_PADDING_PX;
        (0.0..=f32::from(geometry.pixel_size.height_px))
            .contains(&local_y)
            .then_some(local_y)
    }

    fn terminal_scrollbar_local_position(
        &self,
        position: gpui::Point<gpui::Pixels>,
    ) -> Option<(TerminalScrollbarLayout, f32, f32)> {
        let geometry = self.terminal_surface.geometry?;
        let layout = self.terminal_scrollbar_layout()?;
        let local_x =
            f32::from(position.x) - self.sidebar_occupied_width_px() - TERMINAL_PANE_PADDING_PX;
        let local_y = f32::from(position.y) - TERMINAL_PANE_PADDING_PX;
        if local_x < 0.0 || local_y < 0.0 {
            return None;
        }

        let max_x = f32::from(geometry.pixel_size.width_px) + TERMINAL_SCROLLBAR_GUTTER_WIDTH_PX;
        let max_y = f32::from(geometry.pixel_size.height_px);
        if local_x > max_x || local_y > max_y {
            return None;
        }

        Some((layout, local_x, local_y))
    }

    fn terminal_selection_point(
        &self,
        position: gpui::Point<gpui::Pixels>,
    ) -> Option<crate::model::TerminalSelectionPoint> {
        let geometry = self.terminal_surface.geometry?;
        terminal_selection_point_at(
            position,
            self.sidebar_occupied_width_px(),
            TERMINAL_PANE_PADDING_PX,
            geometry,
            self.terminal_line_height_px(),
        )
    }

    fn terminal_mouse_event(
        &self,
        position: gpui::Point<gpui::Pixels>,
        kind: TerminalMouseEventKind,
        button: Option<TerminalMouseButton>,
        modifiers: gpui::Modifiers,
    ) -> Option<TerminalMouseEvent> {
        let geometry = self.terminal_surface.geometry?;
        let (local_x, local_y) = terminal_local_point(
            position,
            self.sidebar_occupied_width_px(),
            TERMINAL_PANE_PADDING_PX,
            geometry,
        )?;

        Some(TerminalMouseEvent {
            kind,
            button,
            x_px: local_x.min(f32::from(geometry.pixel_size.width_px)) as u32,
            y_px: local_y.min(f32::from(geometry.pixel_size.height_px)) as u32,
            modifiers: TerminalInputModifiers {
                control: modifiers.control,
                alt: modifiers.alt,
                shift: modifiers.shift,
                platform: modifiers.platform,
                function: modifiers.function,
            },
        })
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

#[derive(Clone, Copy, Debug, PartialEq)]
struct TerminalScrollbarMouseDownOutcome {
    command: Option<TerminalScrollCommand>,
    drag_state: TerminalScrollbarDragState,
}

fn terminal_scrollbar_is_interactive(
    active_screen: TerminalScreenKind,
    mouse_tracking: bool,
) -> bool {
    !mouse_tracking && matches!(active_screen, TerminalScreenKind::Primary)
}

fn terminal_scrollbar_mouse_down_outcome(
    layout: TerminalScrollbarLayout,
    local_x: f32,
    local_y: f32,
) -> Option<TerminalScrollbarMouseDownOutcome> {
    match layout.hit_test(local_x, local_y)? {
        TerminalScrollbarHit::Thumb => Some(TerminalScrollbarMouseDownOutcome {
            command: None,
            drag_state: layout.drag_state(local_y - layout.thumb_top_px),
        }),
        TerminalScrollbarHit::Track => {
            let drag_state = layout.drag_state(layout.center_grab_offset_y_px());
            Some(TerminalScrollbarMouseDownOutcome {
                command: Some(TerminalScrollCommand::SetOffsetRows(
                    drag_state.offset_for_pointer_y(local_y),
                )),
                drag_state,
            })
        }
    }
}

fn terminal_scrollbar_drag_command(
    drag_state: TerminalScrollbarDragState,
    local_y: f32,
) -> TerminalScrollCommand {
    TerminalScrollCommand::SetOffsetRows(drag_state.offset_for_pointer_y(local_y))
}

fn terminal_local_point(
    position: gpui::Point<gpui::Pixels>,
    sidebar_occupied_width_px: f32,
    terminal_padding_px: f32,
    geometry: seance_terminal::TerminalGeometry,
) -> Option<(f32, f32)> {
    let local_x = f32::from(position.x) - sidebar_occupied_width_px - terminal_padding_px;
    let local_y = f32::from(position.y) - terminal_padding_px;
    if local_x < 0.0 || local_y < 0.0 {
        return None;
    }

    let max_x = f32::from(geometry.pixel_size.width_px);
    let max_y = f32::from(geometry.pixel_size.height_px);
    if local_x > max_x || local_y > max_y {
        return None;
    }

    Some((local_x, local_y))
}

fn terminal_selection_point_at(
    position: gpui::Point<gpui::Pixels>,
    sidebar_occupied_width_px: f32,
    terminal_padding_px: f32,
    geometry: seance_terminal::TerminalGeometry,
    line_height_px: f32,
) -> Option<TerminalSelectionPoint> {
    let (local_x, local_y) = terminal_local_point(
        position,
        sidebar_occupied_width_px,
        terminal_padding_px,
        geometry,
    )?;
    let row = (local_y / line_height_px).floor().max(0.0) as usize;
    let col = (local_x / geometry.cell_width_px as f32).floor().max(0.0) as usize;
    Some(TerminalSelectionPoint { row, col })
}

impl Focusable for SeanceWorkspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SeanceWorkspace {
    fn try_open_terminal_link(
        &mut self,
        event: &MouseDownEvent,
        active_screen: TerminalScreenKind,
    ) -> bool {
        let Some(point) = self.terminal_selection_point(event.position) else {
            return false;
        };
        let Some(session) = self.active_session() else {
            return false;
        };
        let viewport = session.viewport_snapshot();
        let Some(url) = terminal_link_open_request(
            &viewport,
            point,
            active_screen,
            event.button,
            event.modifiers,
        ) else {
            return false;
        };

        if let Err(error) = seance_platform::open_external_url(&url) {
            self.show_toast(error.to_string());
        }
        true
    }

    pub(crate) fn handle_terminal_modifiers_changed(
        &mut self,
        modifiers: gpui::Modifiers,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(hovered_link) = self.terminal_hovered_link.as_mut() else {
            return;
        };

        let modifier_active = terminal_link_open_modifier(modifiers);
        if hovered_link.modifier_active == modifier_active {
            return;
        }

        hovered_link.modifier_active = modifier_active;
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    pub(crate) fn terminal_selected_text(&self) -> Option<String> {
        let selection = self.terminal_selection?;
        let viewport = self.active_session()?.viewport_snapshot();
        selected_terminal_text(&viewport, selection)
    }
}

fn ordered_selection_points(
    selection: crate::model::TerminalSelection,
) -> (
    crate::model::TerminalSelectionPoint,
    crate::model::TerminalSelectionPoint,
) {
    if (selection.anchor.row, selection.anchor.col) <= (selection.focus.row, selection.focus.col) {
        (selection.anchor, selection.focus)
    } else {
        (selection.focus, selection.anchor)
    }
}

fn terminal_row_text_slice(
    row: &seance_terminal::TerminalRow,
    start_col: usize,
    end_col: usize,
) -> String {
    let mut current_col = 0usize;
    let mut text = String::new();

    for cell in &row.cells {
        let cell_width = usize::from(cell.width.max(1));
        let cell_start = current_col;
        let cell_end = current_col + cell_width;
        current_col = cell_end;

        if cell_end <= start_col {
            continue;
        }
        if cell_start >= end_col {
            break;
        }
        text.push_str(&cell.text);
    }

    text
}

#[derive(Default)]
struct TextInputOutcome {
    consumed: bool,
    changed: bool,
    clipboard_write: Option<String>,
}

impl TextInputOutcome {
    fn ignored() -> Self {
        Self::default()
    }
}

fn apply_text_input_event(
    state: &mut crate::TextEditState,
    text: &mut String,
    event: &KeyDownEvent,
    clipboard_text: Option<&str>,
) -> TextInputOutcome {
    let key = event.keystroke.key.as_str();
    let modifiers = event.keystroke.modifiers;

    if is_text_select_all_shortcut(event) {
        state.select_all(text);
        return TextInputOutcome {
            consumed: true,
            changed: false,
            clipboard_write: None,
        };
    }
    if is_text_copy_shortcut(event) {
        return TextInputOutcome {
            consumed: true,
            changed: false,
            clipboard_write: state.copy(text),
        };
    }
    if is_text_cut_shortcut(event) {
        let copied = state.cut(text);
        return TextInputOutcome {
            consumed: true,
            changed: copied.is_some(),
            clipboard_write: copied,
        };
    }
    if is_text_paste_shortcut(event) {
        if let Some(clipboard_text) = clipboard_text {
            state.insert_text(text, clipboard_text);
        }
        return TextInputOutcome {
            consumed: true,
            changed: clipboard_text.is_some(),
            clipboard_write: None,
        };
    }

    match key {
        "left" => {
            state.move_left(text, modifiers.shift);
            TextInputOutcome {
                consumed: true,
                changed: false,
                clipboard_write: None,
            }
        }
        "right" => {
            state.move_right(text, modifiers.shift);
            TextInputOutcome {
                consumed: true,
                changed: false,
                clipboard_write: None,
            }
        }
        "home" => {
            state.move_home(modifiers.shift);
            TextInputOutcome {
                consumed: true,
                changed: false,
                clipboard_write: None,
            }
        }
        "end" => {
            state.move_end(text, modifiers.shift);
            TextInputOutcome {
                consumed: true,
                changed: false,
                clipboard_write: None,
            }
        }
        "backspace" => {
            state.backspace(text);
            TextInputOutcome {
                consumed: true,
                changed: true,
                clipboard_write: None,
            }
        }
        "delete" => {
            state.delete_forward(text);
            TextInputOutcome {
                consumed: true,
                changed: true,
                clipboard_write: None,
            }
        }
        _ => {
            if let Some(ch) = event.keystroke.key_char.as_deref()
                && !modifiers.platform
                && !modifiers.control
                && !modifiers.function
            {
                state.insert_text(text, ch);
                return TextInputOutcome {
                    consumed: true,
                    changed: true,
                    clipboard_write: None,
                };
            }

            TextInputOutcome::ignored()
        }
    }
}

fn apply_text_input_outcome(outcome: &TextInputOutcome, cx: &mut Context<SeanceWorkspace>) {
    if let Some(text) = outcome.clipboard_write.as_ref() {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
    }
}

fn text_input_paste_text(
    event: &KeyDownEvent,
    cx: &mut Context<SeanceWorkspace>,
) -> Option<String> {
    is_text_paste_shortcut(event)
        .then(|| cx.read_from_clipboard().and_then(|item| item.text()))
        .flatten()
}

fn text_input_mutated_key(event: &KeyDownEvent, key_char: Option<&str>) -> bool {
    matches!(event.keystroke.key.as_str(), "backspace" | "delete")
        || is_text_cut_shortcut(event)
        || is_text_paste_shortcut(event)
        || key_char.is_some_and(|_| {
            let modifiers = event.keystroke.modifiers;
            !modifiers.platform && !modifiers.control && !modifiers.function
        })
}

fn text_primary_modifier(modifiers: gpui::Modifiers) -> bool {
    #[cfg(target_os = "macos")]
    {
        modifiers.platform && !modifiers.control
    }
    #[cfg(not(target_os = "macos"))]
    {
        modifiers.control && !modifiers.platform
    }
}

fn is_text_select_all_shortcut(event: &KeyDownEvent) -> bool {
    text_primary_modifier(event.keystroke.modifiers) && event.keystroke.key == "a"
}

fn is_text_copy_shortcut(event: &KeyDownEvent) -> bool {
    text_primary_modifier(event.keystroke.modifiers) && event.keystroke.key == "c"
}

fn is_text_cut_shortcut(event: &KeyDownEvent) -> bool {
    text_primary_modifier(event.keystroke.modifiers) && event.keystroke.key == "x"
}

fn is_text_paste_shortcut(event: &KeyDownEvent) -> bool {
    text_primary_modifier(event.keystroke.modifiers) && event.keystroke.key == "v"
}

fn filter_secure_input_text(target: SecureInputTarget, text: &str) -> String {
    match target {
        SecureInputTarget::HostDraft(HostDraftField::Port)
        | SecureInputTarget::TunnelDraft(TunnelDraftField::ListenPort)
        | SecureInputTarget::TunnelDraft(TunnelDraftField::TargetPort) => text
            .chars()
            .filter(|value| value.is_ascii_digit())
            .collect(),
        _ => text.to_string(),
    }
}

fn terminal_mouse_button(button: MouseButton) -> Option<TerminalMouseButton> {
    match button {
        MouseButton::Left => Some(TerminalMouseButton::Left),
        MouseButton::Right => Some(TerminalMouseButton::Right),
        MouseButton::Middle => Some(TerminalMouseButton::Middle),
        _ => None,
    }
}

fn terminal_link_open_request(
    viewport: &TerminalViewportSnapshot,
    point: TerminalSelectionPoint,
    active_screen: TerminalScreenKind,
    button: MouseButton,
    modifiers: gpui::Modifiers,
) -> Option<String> {
    if button != MouseButton::Left || !terminal_link_open_modifier(modifiers) {
        return None;
    }

    if !matches!(active_screen, TerminalScreenKind::Primary) {
        return None;
    }

    let row = viewport.rows.get(point.row)?;
    terminal_link_at_column(row, point.col)
}

fn terminal_link_open_modifier(modifiers: gpui::Modifiers) -> bool {
    #[cfg(target_os = "macos")]
    {
        modifiers.platform && !modifiers.control && !modifiers.alt && !modifiers.function
    }
    #[cfg(not(target_os = "macos"))]
    {
        modifiers.control && !modifiers.platform && !modifiers.alt && !modifiers.function
    }
}

fn terminal_hovered_link_at_position(
    viewport: &TerminalViewportSnapshot,
    point: TerminalSelectionPoint,
    active_screen: TerminalScreenKind,
    mouse_tracking: bool,
    modifiers: gpui::Modifiers,
) -> Option<TerminalHoveredLink> {
    if !matches!(active_screen, TerminalScreenKind::Primary) || mouse_tracking {
        return None;
    }

    let row = viewport.rows.get(point.row)?;
    let row_revision = viewport
        .row_revisions
        .get(point.row)
        .copied()
        .unwrap_or_default();
    let visible_cols = viewport.cols as usize;
    let link = terminal_links_for_row(row, visible_cols)
        .into_iter()
        .find(|link| link.col_range.contains(&point.col))?;

    Some(TerminalHoveredLink {
        row: point.row,
        row_revision,
        col_range: link.col_range,
        url: link.url,
        modifier_active: terminal_link_open_modifier(modifiers),
    })
}

fn selected_terminal_text(
    viewport: &seance_terminal::TerminalViewportSnapshot,
    selection: crate::model::TerminalSelection,
) -> Option<String> {
    let (start, end) = ordered_selection_points(selection);
    if start == end || start.row >= viewport.rows.len() {
        return None;
    }

    let end_row = end.row.min(viewport.rows.len().saturating_sub(1));
    let mut lines = Vec::new();
    for row_index in start.row..=end_row {
        let Some(row) = viewport.rows.get(row_index) else {
            break;
        };
        let line = if row_index == start.row && row_index == end_row {
            terminal_row_text_slice(row, start.col, end.col)
        } else if row_index == start.row {
            terminal_row_text_slice(row, start.col, usize::MAX)
        } else if row_index == end_row {
            terminal_row_text_slice(row, 0, end.col)
        } else {
            row.plain_text()
        };
        lines.push(line.trim_end_matches(' ').to_string());
    }

    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, mpsc};

    use anyhow::Result;
    use gpui::{Modifiers, MouseButton, point, px};
    use seance_terminal::{
        SessionPerfSnapshot, SessionSummary, TerminalCell, TerminalGeometry, TerminalKeyEvent,
        TerminalMouseEvent, TerminalPaste, TerminalRow, TerminalScreenKind, TerminalScrollCommand,
        TerminalScrollbarState, TerminalSession, TerminalTextEvent, TerminalViewportSnapshot,
    };

    use crate::model::{
        SIDEBAR_DRAG_TARGET_PX, TerminalSelection, TerminalSelectionPoint,
        sidebar_occupied_width_px,
    };

    use super::{
        selected_terminal_text, terminal_hovered_link_at_position, terminal_link_open_modifier,
        terminal_link_open_request, terminal_local_point, terminal_scrollbar_drag_command,
        terminal_scrollbar_is_interactive, terminal_scrollbar_mouse_down_outcome,
        terminal_selection_point_at,
    };

    #[derive(Default)]
    struct RecordingSession {
        scroll_commands: Mutex<Vec<TerminalScrollCommand>>,
    }

    impl RecordingSession {
        fn scroll_commands(&self) -> Vec<TerminalScrollCommand> {
            self.scroll_commands
                .lock()
                .expect("scroll commands poisoned")
                .clone()
        }
    }

    impl TerminalSession for RecordingSession {
        fn id(&self) -> u64 {
            7
        }

        fn title(&self) -> &str {
            "recording"
        }

        fn summary(&self) -> SessionSummary {
            SessionSummary::default()
        }

        fn viewport_snapshot(&self) -> TerminalViewportSnapshot {
            TerminalViewportSnapshot::default()
        }

        fn send_input(&self, _bytes: Vec<u8>) -> Result<()> {
            Ok(())
        }

        fn send_text(&self, _event: TerminalTextEvent) -> Result<()> {
            Ok(())
        }

        fn send_key(&self, _event: TerminalKeyEvent) -> Result<()> {
            Ok(())
        }

        fn send_mouse(&self, _event: TerminalMouseEvent) -> Result<()> {
            Ok(())
        }

        fn paste(&self, _paste: TerminalPaste) -> Result<()> {
            Ok(())
        }

        fn resize(&self, _geometry: TerminalGeometry) -> Result<()> {
            Ok(())
        }

        fn scroll_viewport(&self, command: TerminalScrollCommand) -> Result<()> {
            self.scroll_commands
                .lock()
                .expect("scroll commands poisoned")
                .push(command);
            Ok(())
        }

        fn scroll_to_bottom(&self) -> Result<()> {
            Ok(())
        }

        fn perf_snapshot(&self) -> SessionPerfSnapshot {
            SessionPerfSnapshot::default()
        }

        fn take_notify_rx(&self) -> Option<mpsc::Receiver<()>> {
            None
        }
    }

    fn snapshot(rows: &[&str]) -> TerminalViewportSnapshot {
        TerminalViewportSnapshot {
            rows: rows
                .iter()
                .map(|row| {
                    Arc::new(TerminalRow {
                        cells: row
                            .chars()
                            .map(|ch| TerminalCell {
                                text: ch.to_string(),
                                style: Default::default(),
                                width: 1,
                            })
                            .collect(),
                    })
                })
                .collect(),
            row_revisions: Arc::from(vec![0; rows.len()]),
            cursor: None,
            scrollbar: None,
            revision: 0,
            cols: 80,
            rows_visible: rows.len() as u16,
        }
    }

    #[test]
    fn selected_terminal_text_trims_trailing_spaces_per_line() {
        let viewport = snapshot(&["alpha   ", "beta   "]);
        let selection = TerminalSelection {
            anchor: TerminalSelectionPoint { row: 0, col: 0 },
            focus: TerminalSelectionPoint { row: 1, col: 4 },
        };

        assert_eq!(
            selected_terminal_text(&viewport, selection).as_deref(),
            Some("alpha\nbeta")
        );
    }

    #[test]
    fn selected_terminal_text_handles_reverse_selection() {
        let viewport = snapshot(&["alpha", "beta"]);
        let selection = TerminalSelection {
            anchor: TerminalSelectionPoint { row: 1, col: 2 },
            focus: TerminalSelectionPoint { row: 0, col: 2 },
        };

        assert_eq!(
            selected_terminal_text(&viewport, selection).as_deref(),
            Some("pha\nbe")
        );
    }

    fn geometry() -> TerminalGeometry {
        TerminalGeometry::new(80, 24, 640, 456, 8, 19).expect("terminal geometry")
    }

    fn scrollbar_layout() -> crate::TerminalScrollbarLayout {
        crate::TerminalScrollbarLayout::new(
            TerminalScrollbarState {
                total_rows: 180,
                offset_rows: 60,
                visible_rows: 24,
            },
            geometry(),
        )
        .expect("scrollbar layout")
    }

    #[test]
    fn terminal_local_point_rejects_positions_before_drag_target_and_padding() {
        let occupied_width = sidebar_occupied_width_px(260.0);
        let x_before_terminal = occupied_width + 16.0 - 1.0;

        assert_eq!(
            terminal_local_point(
                point(px(x_before_terminal), px(20.0)),
                occupied_width,
                16.0,
                geometry()
            ),
            None
        );
        assert_eq!(occupied_width, 260.0 + SIDEBAR_DRAG_TARGET_PX);
    }

    #[test]
    fn terminal_local_point_starts_at_zero_after_sidebar_handle_and_padding() {
        let occupied_width = sidebar_occupied_width_px(260.0);

        assert_eq!(
            terminal_local_point(
                point(px(occupied_width + 16.0), px(16.0)),
                occupied_width,
                16.0,
                geometry()
            ),
            Some((0.0, 0.0))
        );
    }

    #[test]
    fn terminal_selection_point_uses_first_cell_after_divider() {
        let occupied_width = sidebar_occupied_width_px(260.0);

        assert_eq!(
            terminal_selection_point_at(
                point(px(occupied_width + 16.0), px(16.0)),
                occupied_width,
                16.0,
                geometry(),
                19.0
            ),
            Some(TerminalSelectionPoint { row: 0, col: 0 })
        );
        assert_eq!(
            terminal_selection_point_at(
                point(px(occupied_width + 16.0 + 8.0), px(16.0 + 19.0)),
                occupied_width,
                16.0,
                geometry(),
                19.0
            ),
            Some(TerminalSelectionPoint { row: 1, col: 1 })
        );
    }

    #[test]
    fn clicking_scrollbar_track_issues_absolute_scroll_command() {
        let layout = scrollbar_layout();
        let session = RecordingSession::default();
        let click_y = layout.thumb_top_px + layout.thumb_height_px + 18.0;
        let outcome =
            terminal_scrollbar_mouse_down_outcome(layout, layout.gutter_left_px + 2.0, click_y)
                .expect("track outcome");

        if let Some(command) = outcome.command {
            session
                .scroll_viewport(command)
                .expect("scroll command should record");
        }

        assert_eq!(
            session.scroll_commands(),
            vec![TerminalScrollCommand::SetOffsetRows(
                outcome.drag_state.offset_for_pointer_y(click_y)
            )]
        );
        assert_eq!(
            outcome.drag_state.grab_offset_y_px,
            layout.center_grab_offset_y_px()
        );
    }

    #[test]
    fn dragging_scrollbar_thumb_emits_absolute_offset_commands() {
        let layout = scrollbar_layout();
        let session = RecordingSession::default();
        let thumb_x = layout.thumb_left_px(layout.active_thumb_width_px()) + 1.0;
        let thumb_y = layout.thumb_top_px + 4.0;
        let drag_state = terminal_scrollbar_mouse_down_outcome(layout, thumb_x, thumb_y)
            .expect("thumb outcome")
            .drag_state;
        let command = terminal_scrollbar_drag_command(
            drag_state,
            layout.track_top_px + layout.track_height_px,
        );

        session
            .scroll_viewport(command)
            .expect("scroll command should record");

        assert_eq!(
            session.scroll_commands(),
            vec![TerminalScrollCommand::SetOffsetRows(layout.max_offset_rows)]
        );
    }

    #[test]
    fn scrollbar_drag_state_is_cleared_when_released() {
        let layout = scrollbar_layout();
        let mut drag_state = Some(layout.drag_state(layout.center_grab_offset_y_px()));

        assert!(drag_state.take().is_some());
        assert_eq!(drag_state, None);
    }

    #[test]
    fn scrollbar_interaction_is_disabled_on_alternate_screen() {
        assert!(!terminal_scrollbar_is_interactive(
            TerminalScreenKind::Alternate,
            false
        ));
    }

    #[test]
    fn scrollbar_interaction_is_disabled_during_mouse_tracking() {
        assert!(!terminal_scrollbar_is_interactive(
            TerminalScreenKind::Primary,
            true
        ));
        assert!(terminal_scrollbar_is_interactive(
            TerminalScreenKind::Primary,
            false
        ));
    }

    #[test]
    fn hovering_link_on_primary_screen_sets_hover_state() {
        let viewport = snapshot(&["visit https://example.com now"]);
        let hovered_link = terminal_hovered_link_at_position(
            &viewport,
            TerminalSelectionPoint { row: 0, col: 10 },
            TerminalScreenKind::Primary,
            false,
            Modifiers::default(),
        )
        .expect("hovered link");

        assert_eq!(hovered_link.row, 0);
        assert_eq!(hovered_link.col_range, 6..25);
        assert_eq!(hovered_link.url, "https://example.com");
        assert!(!hovered_link.modifier_active);
    }

    #[test]
    fn moving_off_link_returns_none() {
        let viewport = snapshot(&["visit https://example.com now"]);

        assert_eq!(
            terminal_hovered_link_at_position(
                &viewport,
                TerminalSelectionPoint { row: 0, col: 2 },
                TerminalScreenKind::Primary,
                false,
                Modifiers::default(),
            ),
            None
        );
    }

    #[test]
    fn mouse_tracking_disables_hovered_link_state() {
        let viewport = snapshot(&["visit https://example.com now"]);

        assert_eq!(
            terminal_hovered_link_at_position(
                &viewport,
                TerminalSelectionPoint { row: 0, col: 10 },
                TerminalScreenKind::Primary,
                true,
                Modifiers::default(),
            ),
            None
        );
    }

    #[test]
    fn alternate_screen_disables_hovered_link_state() {
        let viewport = snapshot(&["visit https://example.com now"]);

        assert_eq!(
            terminal_hovered_link_at_position(
                &viewport,
                TerminalSelectionPoint { row: 0, col: 10 },
                TerminalScreenKind::Alternate,
                false,
                Modifiers::default(),
            ),
            None
        );
    }

    #[test]
    fn hovered_link_tracks_modifier_state() {
        let viewport = snapshot(&["visit https://example.com now"]);
        let hovered_link = terminal_hovered_link_at_position(
            &viewport,
            TerminalSelectionPoint { row: 0, col: 10 },
            TerminalScreenKind::Primary,
            false,
            link_open_modifiers(),
        )
        .expect("hovered link");

        assert!(hovered_link.modifier_active);
    }

    #[test]
    fn modifier_click_on_link_returns_open_request() {
        let viewport = snapshot(&["visit https://example.com now"]);

        assert_eq!(
            terminal_link_open_request(
                &viewport,
                TerminalSelectionPoint { row: 0, col: 10 },
                TerminalScreenKind::Primary,
                MouseButton::Left,
                link_open_modifiers(),
            )
            .as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn plain_click_on_link_does_not_return_open_request() {
        let viewport = snapshot(&["visit https://example.com now"]);

        assert_eq!(
            terminal_link_open_request(
                &viewport,
                TerminalSelectionPoint { row: 0, col: 10 },
                TerminalScreenKind::Primary,
                MouseButton::Left,
                Modifiers::default(),
            ),
            None
        );
    }

    #[test]
    fn link_open_modifier_matches_expected_platform_shortcut() {
        assert!(terminal_link_open_modifier(link_open_modifiers()));
        assert!(!terminal_link_open_modifier(Modifiers::default()));
    }

    fn link_open_modifiers() -> Modifiers {
        #[cfg(target_os = "macos")]
        {
            Modifiers::command()
        }

        #[cfg(not(target_os = "macos"))]
        {
            Modifiers::control()
        }
    }
}
