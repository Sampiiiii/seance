// Owns saved tunnel drafts, tunnel manager rendering, host-linked tunnel actions, and sidebar monitor UI.

use gpui::{Context, Div, FontWeight, MouseButton, div, prelude::*, px};
use seance_ssh::{PortForwardRuntimeSnapshot, PortForwardStatus};
use seance_vault::{PortForwardMode, VaultPortForwardProfile};

use crate::{
    SIDEBAR_FONT_MONO, SIDEBAR_MONO_SIZE_PX, SeanceWorkspace,
    forms::{PendingAction, SecureInputTarget, SecureSection, TunnelDraftField, TunnelDraftState},
    perf::RedrawReason,
    ui_components::{
        danger_button, editor_field_card, primary_button, settings_action_chip, status_badge,
    },
    workspace::{item_scope_key, split_scope_key},
};

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    match bytes {
        0 => "0 B".into(),
        b if b < KB => format!("{b} B"),
        b if b < MB => format!("{:.1} KB", b as f64 / KB as f64),
        b if b < GB => format!("{:.1} MB", b as f64 / MB as f64),
        b => format!("{:.2} GB", b as f64 / GB as f64),
    }
}

/// Returns the current 250ms-aligned animation tick and a phase float 0.0..1.0.
fn anim_tick() -> (u128, f32) {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let tick = ms / 250;
    let phase = ((ms % 1000) as f32) / 1000.0;
    (tick, phase)
}

/// ASCII packet-flow animation: shows data moving through a tunnel pipe.
/// Returns a short mono string that cycles through frames at 250ms.
fn tunnel_flow_art(tick: u128, status: PortForwardStatus) -> &'static str {
    match status {
        PortForwardStatus::Starting => match tick % 4 {
            0 => "╺━━━━━━━╸",
            1 => "━╺━━━━━━╸",
            2 => "━━╺━━━━━╸",
            _ => "━━━╺━━━━╸",
        },
        PortForwardStatus::Running => match tick % 8 {
            0 => "╺──▸·····",
            1 => "·╺──▸····",
            2 => "··╺──▸···",
            3 => "···╺──▸··",
            4 => "····╺──▸·",
            5 => "·····╺──▸",
            6 => "▸·····╺──",
            _ => "─▸·····╺─",
        },
        PortForwardStatus::Failed => match tick % 2 {
            0 => "╳╳╳╳╳╳╳╳╳",
            _ => "─╳─╳─╳─╳─",
        },
    }
}

/// Compact sidebar flow animation — shorter for sidebar width.
fn sidebar_flow_art(tick: u128, status: PortForwardStatus) -> &'static str {
    match status {
        PortForwardStatus::Starting => match tick % 4 {
            0 => "╺━━━╸",
            1 => "━╺━━╸",
            2 => "━━╺━╸",
            _ => "━━━╺╸",
        },
        PortForwardStatus::Running => match tick % 6 {
            0 => "╺─▸··",
            1 => "·╺─▸·",
            2 => "··╺─▸",
            3 => "▸··╺─",
            4 => "─▸··╺",
            _ => "──▸··",
        },
        PortForwardStatus::Failed => match tick % 2 {
            0 => "╳─╳─╳",
            _ => "─╳─╳─",
        },
    }
}

impl SeanceWorkspace {
    pub(crate) fn begin_add_tunnel(&mut self, cx: &mut Context<Self>) {
        let host_scope_key = self.selected_host_id.clone().or_else(|| {
            self.saved_hosts
                .first()
                .map(|host| item_scope_key(&host.vault_id, &host.host.id))
        });
        let Some(host_scope_key) = host_scope_key else {
            self.show_toast("Create or unlock a saved host before adding a tunnel.");
            return;
        };
        self.request_pending_action(
            PendingAction::OpenTunnelDraft {
                tunnel_id: None,
                host_scope_key: Some(host_scope_key),
            },
            cx,
        );
    }

    pub(crate) fn begin_add_tunnel_for_host(
        &mut self,
        host_scope_key: &str,
        cx: &mut Context<Self>,
    ) {
        self.request_pending_action(
            PendingAction::OpenTunnelDraft {
                tunnel_id: None,
                host_scope_key: Some(host_scope_key.to_string()),
            },
            cx,
        );
    }

    pub(crate) fn begin_edit_tunnel(&mut self, tunnel_scope_key: &str, cx: &mut Context<Self>) {
        self.request_pending_action(
            PendingAction::OpenTunnelDraft {
                tunnel_id: Some(tunnel_scope_key.to_string()),
                host_scope_key: None,
            },
            cx,
        );
    }

    pub(crate) fn activate_tunnel_draft(
        &mut self,
        tunnel_scope_key: Option<&str>,
        host_scope_key: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.surface = crate::forms::WorkspaceSurface::Secure;
        self.secure.section = SecureSection::Tunnels;
        self.refresh_saved_hosts();
        self.refresh_vault_cache();
        self.secure.host_draft = None;
        self.secure.credential_draft = None;

        self.secure.tunnel_draft = if let Some(tunnel_scope_key) = tunnel_scope_key {
            let Some((vault_id, tunnel_id)) = split_scope_key(tunnel_scope_key) else {
                self.show_toast("Tunnel scope is invalid.");
                return;
            };
            match self.backend.load_port_forward(vault_id, tunnel_id) {
                Ok(Some(port_forward)) => {
                    let mut draft = TunnelDraftState::from_port_forward(port_forward);
                    draft.vault_id = Some(vault_id.to_string());
                    if let Some(host) = self.saved_hosts.iter().find(|host| {
                        host.vault_id == vault_id
                            && host.host.id == draft.host_id.as_deref().unwrap_or_default()
                    }) {
                        draft.host_label = host.host.label.clone();
                    }
                    self.secure.selected_tunnel_id = Some(item_scope_key(vault_id, tunnel_id));
                    Some(draft)
                }
                Ok(None) => {
                    self.show_toast("Saved tunnel not found.");
                    None
                }
                Err(err) => {
                    self.show_toast(err.to_string());
                    None
                }
            }
        } else {
            let host_scope_key = host_scope_key
                .map(str::to_string)
                .or_else(|| self.selected_host_id.clone())
                .or_else(|| {
                    self.saved_hosts
                        .first()
                        .map(|host| item_scope_key(&host.vault_id, &host.host.id))
                });
            let Some(host_scope_key) = host_scope_key else {
                self.show_toast("Create or unlock a saved host before adding a tunnel.");
                return;
            };
            let Some((vault_id, host_id)) = split_scope_key(&host_scope_key) else {
                self.show_toast("Host scope is invalid.");
                return;
            };
            let Some(host) = self
                .saved_hosts
                .iter()
                .find(|host| host.vault_id == vault_id && host.host.id == host_id)
                .cloned()
            else {
                self.show_toast("Saved host not found.");
                return;
            };
            let mut draft = TunnelDraftState::blank();
            draft.vault_id = Some(vault_id.to_string());
            draft.host_id = Some(host.host.id.clone());
            draft.host_label = host.host.label;
            self.secure.selected_tunnel_id = None;
            Some(draft)
        };

        if self.secure.tunnel_draft.is_some() {
            self.focus_secure_input_target(SecureInputTarget::TunnelDraft(TunnelDraftField::Label));
        }
        self.confirm_dialog = None;
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn save_tunnel_draft(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.secure.tunnel_draft.as_ref() else {
            return;
        };
        let errors = draft.validation_errors();
        if !errors.is_empty() {
            if let Some(draft) = self.secure.tunnel_draft.as_mut() {
                draft.error = Some(errors.join(" "));
            }
            cx.notify();
            return;
        }

        let Some(vault_id) = draft.vault_id.clone() else {
            self.show_toast("Choose an unlocked vault before saving this tunnel.");
            return;
        };
        let Some(host_id) = draft.host_id.clone() else {
            self.show_toast("Choose a host before saving this tunnel.");
            return;
        };

        let port_forward = VaultPortForwardProfile {
            id: draft.port_forward_id.clone().unwrap_or_default(),
            host_id,
            label: draft.label.trim().to_string(),
            mode: draft.mode,
            listen_address: draft.listen_address.trim().to_string(),
            listen_port: draft.parsed_listen_port().unwrap_or_default(),
            target_address: draft.target_address.trim().to_string(),
            target_port: draft.parsed_target_port().unwrap_or_default(),
            notes: (!draft.notes.trim().is_empty()).then(|| draft.notes.trim().to_string()),
        };

        match self.backend.save_port_forward(&vault_id, port_forward) {
            Ok(summary) => {
                self.refresh_vault_ui(cx);
                self.secure.selected_tunnel_id =
                    Some(item_scope_key(&summary.vault_id, &summary.port_forward.id));
                if let Some(draft) = self.secure.tunnel_draft.as_mut() {
                    draft.vault_id = Some(summary.vault_id.clone());
                    draft.port_forward_id = Some(summary.port_forward.id.clone());
                    draft.host_id = Some(summary.host_id.clone());
                    draft.host_label = summary.host_label.clone();
                    draft.dirty = false;
                    draft.error = None;
                }
                self.show_toast(format!("Saved tunnel '{}'.", summary.port_forward.label));
            }
            Err(err) => {
                if let Some(draft) = self.secure.tunnel_draft.as_mut() {
                    draft.error = Some(err.to_string());
                }
            }
        }
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn delete_saved_tunnel(&mut self, tunnel_scope_key: &str, cx: &mut Context<Self>) {
        let Some((vault_id, tunnel_id)) = split_scope_key(tunnel_scope_key) else {
            self.show_toast("Tunnel scope is invalid.");
            return;
        };
        let was_running = self.tunnel_runtime(tunnel_scope_key).is_some();
        match self.backend.delete_port_forward(vault_id, tunnel_id) {
            Ok(true) => {
                self.refresh_vault_ui(cx);
                if was_running {
                    let _ = self.backend.stop_port_forward(tunnel_scope_key);
                    self.show_toast(
                        "Tunnel deleted from the vault. The live tunnel is stopping in the background.",
                    );
                } else {
                    self.show_toast("Tunnel removed from the vault.");
                }
            }
            Ok(false) => self.show_toast("Tunnel already removed."),
            Err(err) => self.show_toast(err.to_string()),
        }
        cx.notify();
    }

    pub(crate) fn start_saved_tunnel(&mut self, tunnel_scope_key: &str, cx: &mut Context<Self>) {
        let Some((vault_id, tunnel_id)) = split_scope_key(tunnel_scope_key) else {
            self.show_toast("Tunnel scope is invalid.");
            return;
        };
        match self.backend.build_port_forward_request(vault_id, tunnel_id) {
            Ok(request) => match self.backend.start_port_forward(request) {
                Ok(snapshot) => self.show_toast(format!("Starting tunnel '{}'.", snapshot.label)),
                Err(err) => self.show_toast(err.to_string()),
            },
            Err(err) => self.show_toast(err.to_string()),
        }
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn stop_saved_tunnel(&mut self, tunnel_scope_key: &str, cx: &mut Context<Self>) {
        if self.backend.stop_port_forward(tunnel_scope_key) {
            self.show_toast("Stopping tunnel…");
        } else {
            self.show_toast("Tunnel is not running.");
        }
        self.perf_overlay.mark_input(RedrawReason::UiRefresh);
        cx.notify();
    }

    fn tunnel_runtime(&self, tunnel_scope_key: &str) -> Option<&PortForwardRuntimeSnapshot> {
        self.active_port_forwards
            .iter()
            .find(|snapshot| snapshot.id == tunnel_scope_key)
    }

    pub(crate) fn live_tunnel_is_saved(&self, tunnel_scope_key: &str) -> bool {
        split_scope_key(tunnel_scope_key)
            .map(|(vault_id, tunnel_id)| self.saved_tunnel_exists(vault_id, tunnel_id))
            .unwrap_or(false)
    }

    fn tunnel_matches_query(&self, tunnel: &seance_core::VaultScopedPortForwardSummary) -> bool {
        let query = self.secure.tunnel_search.trim().to_lowercase();
        query.is_empty()
            || tunnel.port_forward.label.to_lowercase().contains(&query)
            || tunnel.host_label.to_lowercase().contains(&query)
            || tunnel.vault_name.to_lowercase().contains(&query)
            || tunnel
                .port_forward
                .listen_address
                .to_lowercase()
                .contains(&query)
            || tunnel
                .port_forward
                .target_address
                .to_lowercase()
                .contains(&query)
    }

    pub(crate) fn render_tunnel_list_panel(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let panel = div()
            .w(px(320.0))
            .h_full()
            .p_4()
            .rounded_xl()
            .bg(t.glass_tint)
            .border_1()
            .border_color(t.glass_border)
            .flex()
            .flex_col()
            .gap_3()
            .child(
                self.render_search_card(
                    "Search tunnels",
                    &self.secure.tunnel_search,
                    self.secure.input_target == SecureInputTarget::TunnelSearch,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.focus_secure_input_target(SecureInputTarget::TunnelSearch);
                        cx.notify();
                    }),
                ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(settings_action_chip("new tunnel", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.begin_add_tunnel(cx);
                        }),
                    ))
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child(format!("{} saved", self.cached_port_forwards.len())),
                    ),
            );

        let mut rows = div().flex().flex_col().gap(px(8.0));
        for tunnel in self
            .cached_port_forwards
            .iter()
            .filter(|tunnel| self.tunnel_matches_query(tunnel))
        {
            let tunnel_scope_key = item_scope_key(&tunnel.vault_id, &tunnel.port_forward.id);
            let runtime = self.tunnel_runtime(&tunnel_scope_key);
            let selected =
                self.secure.selected_tunnel_id.as_deref() == Some(tunnel_scope_key.as_str());
            let mode_glyph = match tunnel.port_forward.mode {
                PortForwardMode::Local => "L",
                PortForwardMode::Remote => "R",
            };
            let (status_dot_color, status_label) = match runtime.map(|r| r.status) {
                Some(PortForwardStatus::Starting) => (t.warning, "starting"),
                Some(PortForwardStatus::Running) => (t.success, "live"),
                Some(PortForwardStatus::Failed) => (t.danger, "failed"),
                None => (t.text_ghost, "idle"),
            };
            rows = rows.child(
                div()
                    .p(px(10.0))
                    .rounded_lg()
                    .border_1()
                    .cursor_pointer()
                    .when(selected, |el| {
                        el.border_color(t.accent).bg(t.accent_glow).border_l_2()
                    })
                    .when(!selected, |el| {
                        el.border_color(t.glass_border)
                            .bg(t.glass_tint)
                            .hover(|s| s.bg(t.glass_hover))
                    })
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    // Row 1: status dot + label + status badge
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.0))
                                    .child(
                                        div()
                                            .w(px(7.0))
                                            .h(px(7.0))
                                            .rounded_full()
                                            .bg(status_dot_color),
                                    )
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(t.text_primary)
                                            .line_clamp(1)
                                            .child(tunnel.port_forward.label.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .px(px(6.0))
                                    .py(px(2.0))
                                    .rounded(px(4.0))
                                    .bg(status_dot_color.opacity(0.12))
                                    .text_color(status_dot_color)
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(9.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(status_label),
                            ),
                    )
                    // Row 2: port flow visualization
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .px(px(4.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .bg(t.accent.opacity(0.15))
                                    .text_color(t.accent)
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(9.0))
                                    .font_weight(FontWeight::BOLD)
                                    .child(mode_glyph),
                            )
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                    .text_color(t.text_muted)
                                    .child(format!(":{}", tunnel.port_forward.listen_port)),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(t.text_ghost)
                                    .child("→"),
                            )
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                    .text_color(t.text_muted)
                                    .child(format!(":{}", tunnel.port_forward.target_port)),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .text_color(t.text_ghost)
                                    .line_clamp(1)
                                    .child(tunnel.host_label.clone()),
                            ),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.begin_edit_tunnel(&tunnel_scope_key, cx);
                        }),
                    ),
            );
        }

        panel.child(
            div()
                .id("tunnel-list-scroll")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(rows),
        )
    }

    pub(crate) fn render_tunnel_detail_panel(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let Some(draft) = self.secure.tunnel_draft.as_ref() else {
            return self.render_placeholder_panel(
                "Select a tunnel",
                "Choose a saved tunnel or create a new one.",
            );
        };
        let tunnel_scope_key = draft
            .port_forward_id
            .as_ref()
            .zip(draft.vault_id.as_ref())
            .map(|(id, vault_id)| item_scope_key(vault_id, id));
        let runtime = tunnel_scope_key
            .as_deref()
            .and_then(|scope_key| self.tunnel_runtime(scope_key));
        let live_deleted = tunnel_scope_key
            .as_deref()
            .is_some_and(|scope_key| runtime.is_some() && !self.live_tunnel_is_saved(scope_key));

        let field = |field: TunnelDraftField, value: String| {
            let selected = self.secure.input_target == SecureInputTarget::TunnelDraft(field);
            editor_field_card(
                field.title(),
                value,
                selected,
                selected.then_some(&self.secure_text_input),
                &t,
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.focus_secure_input_target(SecureInputTarget::TunnelDraft(field));
                    cx.notify();
                }),
            )
        };

        // --- Mode segmented control ---
        let mode_segment = |label: &'static str, mode: PortForwardMode| {
            let is_active = draft.mode == mode;
            div()
                .flex_1()
                .px(px(12.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .text_center()
                .when(is_active, |el| el.bg(t.accent).text_color(t.bg_void))
                .when(!is_active, |el| {
                    el.bg(t.glass_tint)
                        .text_color(t.text_muted)
                        .hover(|s| s.bg(t.glass_hover).text_color(t.text_primary))
                })
                .child(label)
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(draft) = this.secure.tunnel_draft.as_mut() {
                            draft.mode = mode;
                            draft.dirty = true;
                        }
                        cx.notify();
                    }),
                )
        };

        let mode_control = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.text_ghost)
                    .child("DIRECTION"),
            )
            .child(
                div()
                    .flex()
                    .gap(px(2.0))
                    .p(px(2.0))
                    .rounded(px(8.0))
                    .bg(t.glass_border)
                    .child(mode_segment("Local  ↓", PortForwardMode::Local))
                    .child(mode_segment("Remote  ↑", PortForwardMode::Remote)),
            );

        // --- Source / Destination paired cards ---
        let source_card = div()
            .flex_1()
            .min_w(px(200.0))
            .p(px(12.0))
            .rounded_lg()
            .bg(t.glass_tint)
            .border_1()
            .border_color(t.glass_border)
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text_ghost)
                            .child("SOURCE"),
                    )
                    .child(
                        div()
                            .px(px(4.0))
                            .py(px(1.0))
                            .rounded(px(3.0))
                            .bg(t.accent.opacity(0.12))
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(9.0))
                            .text_color(t.accent)
                            .font_weight(FontWeight::BOLD)
                            .child(if draft.mode == PortForwardMode::Local {
                                "listen"
                            } else {
                                "connect"
                            }),
                    ),
            )
            .child(field(
                TunnelDraftField::ListenAddress,
                draft.listen_address.clone(),
            ))
            .child(field(
                TunnelDraftField::ListenPort,
                draft.listen_port.clone(),
            ));

        let dest_card = div()
            .flex_1()
            .min_w(px(200.0))
            .p(px(12.0))
            .rounded_lg()
            .bg(t.glass_tint)
            .border_1()
            .border_color(t.glass_border)
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text_ghost)
                            .child("DESTINATION"),
                    )
                    .child(
                        div()
                            .px(px(4.0))
                            .py(px(1.0))
                            .rounded(px(3.0))
                            .bg(t.success.opacity(0.12))
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(9.0))
                            .text_color(t.success)
                            .font_weight(FontWeight::BOLD)
                            .child(if draft.mode == PortForwardMode::Local {
                                "forward"
                            } else {
                                "accept"
                            }),
                    ),
            )
            .child(field(
                TunnelDraftField::TargetAddress,
                draft.target_address.clone(),
            ))
            .child(field(
                TunnelDraftField::TargetPort,
                draft.target_port.clone(),
            ));

        let flow_arrow = div()
            .flex()
            .items_center()
            .justify_center()
            .py(px(8.0))
            .px(px(4.0))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(2.0))
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(11.0))
                            .text_color(t.accent.opacity(0.7))
                            .child(tunnel_flow_art(
                                anim_tick().0,
                                runtime
                                    .map(|r| r.status)
                                    .unwrap_or(PortForwardStatus::Starting),
                            )),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(8.0))
                            .text_color(t.text_ghost)
                            .child("via ssh"),
                    ),
            );

        let port_flow_section = div()
            .flex()
            .flex_wrap()
            .gap(px(0.0))
            .child(source_card)
            .child(flow_arrow)
            .child(dest_card);

        // --- Assemble the panel ---
        // Outer shell: fixed height, flex column, buttons pinned at bottom.
        let mut scrollable = div().flex().flex_col().gap_4();

        // Header
        scrollable = scrollable.child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::BOLD)
                                .text_color(t.text_primary)
                                .child(if draft.port_forward_id.is_some() {
                                    "Edit Tunnel"
                                } else {
                                    "New Tunnel"
                                }),
                        )
                        .child(
                            div()
                                .px(px(8.0))
                                .py(px(3.0))
                                .rounded(px(6.0))
                                .bg(t.glass_tint)
                                .border_1()
                                .border_color(t.glass_border)
                                .flex()
                                .items_center()
                                .gap(px(5.0))
                                .child(
                                    div()
                                        .w(px(5.0))
                                        .h(px(5.0))
                                        .rounded_full()
                                        .bg(t.accent.opacity(0.6)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(t.text_muted)
                                        .child(draft.host_label.clone()),
                                ),
                        ),
                )
                .child(if draft.dirty {
                    status_badge("unsaved", t.warning, &t)
                } else {
                    status_badge("saved", t.success, &t)
                }),
        );

        if let Some(error) = draft.error.as_ref() {
            scrollable = scrollable.child(self.render_banner(error, true));
        }
        if live_deleted {
            scrollable = scrollable.child(self.render_banner(
                "This tunnel was deleted from the vault. The live runtime remains visible until it stops.",
                true,
            ));
        }

        scrollable = scrollable
            // Label
            .child(field(TunnelDraftField::Label, draft.label.clone()))
            // Mode toggle
            .child(mode_control)
            // Source → Destination flow
            .child(port_flow_section)
            // Notes
            .child(field(TunnelDraftField::Notes, draft.notes.clone()))
            // Runtime
            .child(self.render_section_card(
                "Runtime",
                self.render_tunnel_runtime_card(runtime, live_deleted),
            ));

        // Action buttons — pinned at the bottom, outside the scroll area.
        let mut actions = div().flex().items_center().gap(px(8.0));
        if let Some(scope_key) = tunnel_scope_key.as_ref() {
            let delete_scope_key = scope_key.clone();
            let toggle_scope_key = scope_key.clone();
            let running =
                runtime.is_some_and(|runtime| runtime.status != PortForwardStatus::Failed);
            actions = actions
                .child(danger_button("delete", &t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.delete_saved_tunnel(&delete_scope_key, cx);
                    }),
                ))
                .child(
                    settings_action_chip(if running { "stop" } else { "start" }, &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if running {
                                this.stop_saved_tunnel(&toggle_scope_key, cx);
                            } else {
                                this.start_saved_tunnel(&toggle_scope_key, cx);
                            }
                        }),
                    ),
                );
        }

        let action_bar = div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(8.0))
            .pt_3()
            .border_t_1()
            .border_color(t.glass_border)
            .child(actions)
            .child(div().flex_1())
            .child(
                primary_button("save tunnel", draft.can_save(), &t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.save_tunnel_draft(cx);
                    }),
                ),
            );

        // Outer panel shell: scroll body + pinned action bar.
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
            .gap_3()
            .child(
                div()
                    .id("tunnel-detail-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(scrollable),
            )
            .child(action_bar)
    }

    pub(crate) fn render_host_tunnels_section(&self, cx: &mut Context<Self>) -> Div {
        let t = self.theme();
        let Some(draft) = self.secure.host_draft.as_ref() else {
            return div();
        };
        let Some(vault_id) = draft.vault_id.as_deref() else {
            return div();
        };
        let Some(host_id) = draft.host_id.as_deref() else {
            return div();
        };

        let host_scope_key = item_scope_key(vault_id, host_id);
        let mut rows = div().flex().flex_col().gap(px(8.0));
        let mut any = false;
        for tunnel in self
            .cached_port_forwards
            .iter()
            .filter(|tunnel| tunnel.vault_id == vault_id && tunnel.host_id == host_id)
        {
            any = true;
            let tunnel_scope_key = item_scope_key(vault_id, &tunnel.port_forward.id);
            let runtime = self.tunnel_runtime(&tunnel_scope_key);
            let mode_glyph = match tunnel.port_forward.mode {
                PortForwardMode::Local => "L",
                PortForwardMode::Remote => "R",
            };
            let (status_color, status_label) = match runtime.map(|s| s.status) {
                Some(PortForwardStatus::Starting) => (t.warning, "starting"),
                Some(PortForwardStatus::Running) => (t.success, "live"),
                Some(PortForwardStatus::Failed) => (t.danger, "failed"),
                None => (t.text_ghost, "idle"),
            };
            rows = rows.child(
                div()
                    .p(px(8.0))
                    .rounded_lg()
                    .bg(t.glass_tint)
                    .border_1()
                    .border_color(t.glass_border)
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    // Status dot
                    .child(div().w(px(7.0)).h(px(7.0)).rounded_full().bg(status_color))
                    // Tunnel info
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(t.text_primary)
                                    .line_clamp(1)
                                    .child(tunnel.port_forward.label.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .px(px(3.0))
                                            .rounded(px(2.0))
                                            .bg(t.accent.opacity(0.15))
                                            .font_family(SIDEBAR_FONT_MONO)
                                            .text_size(px(9.0))
                                            .text_color(t.accent)
                                            .font_weight(FontWeight::BOLD)
                                            .child(mode_glyph),
                                    )
                                    .child(
                                        div()
                                            .font_family(SIDEBAR_FONT_MONO)
                                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                            .text_color(t.text_muted)
                                            .child(format!(
                                                ":{} → :{}",
                                                tunnel.port_forward.listen_port,
                                                tunnel.port_forward.target_port
                                            )),
                                    )
                                    .child(
                                        div()
                                            .px(px(4.0))
                                            .py(px(1.0))
                                            .rounded(px(3.0))
                                            .bg(status_color.opacity(0.12))
                                            .font_family(SIDEBAR_FONT_MONO)
                                            .text_size(px(8.0))
                                            .text_color(status_color)
                                            .child(status_label),
                                    ),
                            ),
                    )
                    // Action buttons
                    .child(
                        div()
                            .flex()
                            .gap(px(4.0))
                            .child(settings_action_chip("edit", &t).on_mouse_down(
                                MouseButton::Left,
                                cx.listener({
                                    let tunnel_scope_key = tunnel_scope_key.clone();
                                    move |this, _, _, cx| {
                                        this.begin_edit_tunnel(&tunnel_scope_key, cx);
                                    }
                                }),
                            ))
                            .child(
                                settings_action_chip(
                                    if runtime.is_some() { "stop" } else { "start" },
                                    &t,
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener({
                                        let tunnel_scope_key = tunnel_scope_key.clone();
                                        let running = runtime.is_some();
                                        move |this, _, _, cx| {
                                            if running {
                                                this.stop_saved_tunnel(&tunnel_scope_key, cx);
                                            } else {
                                                this.start_saved_tunnel(&tunnel_scope_key, cx);
                                            }
                                        }
                                    }),
                                ),
                            ),
                    ),
            );
        }

        self.render_section_card(
            "Port Forwarding",
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(settings_action_chip("new linked tunnel", &t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.begin_add_tunnel_for_host(&host_scope_key, cx);
                    }),
                ))
                .child(if any {
                    rows
                } else {
                    div()
                        .text_sm()
                        .text_color(t.text_muted)
                        .child("No port forwarding rules for this host yet.")
                }),
        )
    }

    fn render_tunnel_runtime_card(
        &self,
        runtime: Option<&PortForwardRuntimeSnapshot>,
        live_deleted: bool,
    ) -> Div {
        let t = self.theme();
        let Some(runtime) = runtime else {
            return div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .w(px(8.0))
                        .h(px(8.0))
                        .rounded_full()
                        .border_1()
                        .border_color(t.text_ghost),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(t.text_ghost)
                        .child("Tunnel is stopped"),
                );
        };

        let (tick, _) = anim_tick();
        let (status_label, status_color) = match runtime.status {
            PortForwardStatus::Starting => ("STARTING", t.warning),
            PortForwardStatus::Running => ("RUNNING", t.success),
            PortForwardStatus::Failed => ("FAILED", t.danger),
        };

        let pulse = match tick % 4 {
            0 => 1.0_f32,
            1 => 0.7,
            2 => 0.4,
            _ => 0.7,
        };

        let flow_art = tunnel_flow_art(tick, runtime.status);

        let stat_pill = |label: &str, value: String, color: gpui::Hsla| {
            div()
                .flex_1()
                .min_w(px(72.0))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(2.0))
                .px(px(8.0))
                .py(px(6.0))
                .rounded(px(6.0))
                .bg(color.opacity(0.06))
                .border_1()
                .border_color(color.opacity(0.12))
                .child(
                    div()
                        .font_family(SIDEBAR_FONT_MONO)
                        .text_size(px(14.0))
                        .font_weight(FontWeight::BOLD)
                        .text_color(t.text_primary)
                        .child(value),
                )
                .child(
                    div()
                        .font_family(SIDEBAR_FONT_MONO)
                        .text_size(px(8.0))
                        .text_color(color.opacity(0.7))
                        .child(label.to_string()),
                )
        };

        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(10.0))
            // Status row with animated flow
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .w(px(10.0))
                            .h(px(10.0))
                            .rounded_full()
                            .bg(status_color.opacity(pulse)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(SIDEBAR_FONT_MONO)
                            .font_weight(FontWeight::BOLD)
                            .text_color(status_color)
                            .child(status_label),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(11.0))
                            .text_color(status_color.opacity(0.6))
                            .child(flow_art),
                    ),
            )
            // Stats row — wraps responsively
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(6.0))
                    .child(stat_pill(
                        "CONNS",
                        runtime.active_connections.to_string(),
                        t.accent,
                    ))
                    .child(stat_pill("▼ IN", format_bytes(runtime.bytes_in), t.success))
                    .child(stat_pill(
                        "▲ OUT",
                        format_bytes(runtime.bytes_out),
                        t.warning,
                    )),
            );

        if live_deleted {
            card = card.child(
                div()
                    .text_xs()
                    .text_color(t.warning)
                    .child("Rule deleted from vault — runtime active until stopped."),
            );
        }

        if let Some(error) = runtime.last_error.as_ref() {
            card = card.child(
                div()
                    .px(px(8.0))
                    .py(px(4.0))
                    .rounded(px(6.0))
                    .bg(t.danger.opacity(0.08))
                    .border_1()
                    .border_color(t.danger.opacity(0.2))
                    .text_xs()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_color(t.danger)
                    .child(error.clone()),
            );
        }
        card
    }

    pub(crate) fn render_tunnel_monitor_section(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        if self.active_port_forwards.is_empty() {
            return div();
        }

        let (tick, _phase) = anim_tick();

        // Pulse opacity cycle for status dots
        let pulse = match tick % 4 {
            0 => 1.0_f32,
            1 => 0.6,
            2 => 0.3,
            _ => 0.6,
        };

        let mut section =
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(self.render_sidebar_section_heading(
                    "tunnels",
                    self.active_port_forwards.len().to_string(),
                ));

        for tunnel in &self.active_port_forwards {
            let scope_key = tunnel.id.clone();
            let is_saved = self.live_tunnel_is_saved(&scope_key);
            let mode = match tunnel.mode {
                seance_ssh::SshPortForwardMode::Local => "L",
                seance_ssh::SshPortForwardMode::Remote => "R",
            };
            let (status_color, should_pulse) = match tunnel.status {
                PortForwardStatus::Starting => (theme.warning, true),
                PortForwardStatus::Running => (theme.success, true),
                PortForwardStatus::Failed => (theme.danger, false),
            };
            let dot_opacity = if should_pulse { pulse } else { 1.0 };
            let flow_art = sidebar_flow_art(tick, tunnel.status);
            let total_bytes = tunnel.bytes_in + tunnel.bytes_out;

            section = section.child(
                self.sidebar_row_shell(false)
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .child(
                        // Top row: dot + mode + label + stop
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            // Status dot
                            .child(
                                div()
                                    .w(px(7.0))
                                    .h(px(7.0))
                                    .rounded_full()
                                    .bg(status_color.opacity(dot_opacity)),
                            )
                            // Mode pill
                            .child(
                                div()
                                    .px(px(3.0))
                                    .rounded(px(2.0))
                                    .bg(theme.accent.opacity(0.15))
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(8.0))
                                    .text_color(theme.accent)
                                    .font_weight(FontWeight::BOLD)
                                    .child(mode),
                            )
                            // Label
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text_primary)
                                    .line_clamp(1)
                                    .child(tunnel.label.clone()),
                            )
                            .when(!is_saved, |el| {
                                el.child(
                                    div()
                                        .px(px(3.0))
                                        .rounded(px(2.0))
                                        .bg(theme.warning.opacity(0.15))
                                        .font_family(SIDEBAR_FONT_MONO)
                                        .text_size(px(7.0))
                                        .text_color(theme.warning)
                                        .child("orphan"),
                                )
                            })
                            // Stop button
                            .child(
                                div()
                                    .px(px(4.0))
                                    .py(px(1.0))
                                    .rounded(px(3.0))
                                    .border_1()
                                    .border_color(theme.danger.opacity(0.25))
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(8.0))
                                    .text_color(theme.danger.opacity(0.6))
                                    .cursor_pointer()
                                    .hover(|s| {
                                        s.bg(theme.danger.opacity(0.1))
                                            .text_color(theme.danger)
                                            .border_color(theme.danger.opacity(0.5))
                                    })
                                    .child("×")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, _, cx| {
                                            this.stop_saved_tunnel(&scope_key, cx);
                                        }),
                                    ),
                            ),
                    )
                    .child(
                        // Bottom row: port flow + ASCII art + throughput
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .ml(px(13.0)) // align under label, past dot
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                    .text_color(theme.text_muted)
                                    .child(format!(":{}", tunnel.listen_port)),
                            )
                            // Animated flow art
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(9.0))
                                    .text_color(status_color.opacity(0.7))
                                    .child(flow_art),
                            )
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                    .text_color(theme.text_muted)
                                    .child(format!(":{}", tunnel.target_port)),
                            )
                            .child(div().flex_1())
                            .when(total_bytes > 0, |el| {
                                el.child(
                                    div()
                                        .font_family(SIDEBAR_FONT_MONO)
                                        .text_size(px(8.0))
                                        .text_color(theme.text_ghost)
                                        .child(format_bytes(total_bytes)),
                                )
                            }),
                    ),
            );
        }

        section
    }
}
