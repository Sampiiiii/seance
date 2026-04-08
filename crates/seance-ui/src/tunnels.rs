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
            self.secure.input_target = SecureInputTarget::TunnelDraft(TunnelDraftField::Label);
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

    fn tunnel_runtime_hint(runtime: Option<&PortForwardRuntimeSnapshot>) -> &'static str {
        runtime
            .map(|snapshot| match snapshot.status {
                PortForwardStatus::Starting => "starting",
                PortForwardStatus::Running => "running",
                PortForwardStatus::Failed => "failed",
            })
            .unwrap_or("stopped")
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
                        this.secure.input_target = SecureInputTarget::TunnelSearch;
                        cx.notify();
                    }),
                ),
            )
            .child(settings_action_chip("new tunnel", &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.begin_add_tunnel(cx);
                }),
            ));

        let mut rows = div().flex().flex_col().gap(px(6.0));
        for tunnel in self
            .cached_port_forwards
            .iter()
            .filter(|tunnel| self.tunnel_matches_query(tunnel))
        {
            let tunnel_scope_key = item_scope_key(&tunnel.vault_id, &tunnel.port_forward.id);
            let runtime = self.tunnel_runtime(&tunnel_scope_key);
            let selected =
                self.secure.selected_tunnel_id.as_deref() == Some(tunnel_scope_key.as_str());
            let mode = match tunnel.port_forward.mode {
                PortForwardMode::Local => "local",
                PortForwardMode::Remote => "remote",
            };
            let runtime_hint = Self::tunnel_runtime_hint(runtime);
            rows = rows.child(
                self.render_list_row(
                    &tunnel.port_forward.label,
                    &format!(
                        "{mode}  {}:{} -> {}:{}  [{} / {}]  {runtime_hint}",
                        tunnel.port_forward.listen_address,
                        tunnel.port_forward.listen_port,
                        tunnel.port_forward.target_address,
                        tunnel.port_forward.target_port,
                        tunnel.host_label,
                        tunnel.vault_name
                    ),
                    selected,
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.begin_edit_tunnel(&tunnel_scope_key, cx);
                    }),
                ),
            );
        }

        panel.child(div().flex_1().child(rows))
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
            editor_field_card(field.title(), value, selected, &t).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.secure.input_target = SecureInputTarget::TunnelDraft(field);
                    if let Some(draft) = this.secure.tunnel_draft.as_mut() {
                        draft.selected_field = field;
                    }
                    cx.notify();
                }),
            )
        };

        let mode_label = match draft.mode {
            PortForwardMode::Local => "local (L)".to_string(),
            PortForwardMode::Remote => "remote (R)".to_string(),
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
                                    .child(if draft.port_forward_id.is_some() {
                                        "Edit Tunnel"
                                    } else {
                                        "New Tunnel"
                                    }),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(t.text_muted)
                                    .child(format!("Linked host: {}", draft.host_label)),
                            ),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_color(if draft.dirty { t.warning } else { t.accent })
                            .child(if draft.dirty {
                                status_badge("unsaved", t.warning, &t)
                            } else {
                                status_badge("saved", t.success, &t)
                            }),
                    ),
            );

        if let Some(error) = draft.error.as_ref() {
            panel = panel.child(self.render_banner(error, true));
        }
        if live_deleted {
            panel = panel.child(self.render_banner(
                "This tunnel was deleted from the vault. The live runtime remains visible until it stops.",
                true,
            ));
        }

        panel = panel
            .child(
                self.render_section_card(
                    "Rule",
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(field(TunnelDraftField::Label, draft.label.clone()))
                        .child(field(TunnelDraftField::Mode, mode_label))
                        .child(field(
                            TunnelDraftField::ListenAddress,
                            draft.listen_address.clone(),
                        ))
                        .child(field(
                            TunnelDraftField::ListenPort,
                            draft.listen_port.clone(),
                        ))
                        .child(field(
                            TunnelDraftField::TargetAddress,
                            draft.target_address.clone(),
                        ))
                        .child(field(
                            TunnelDraftField::TargetPort,
                            draft.target_port.clone(),
                        ))
                        .child(field(TunnelDraftField::Notes, draft.notes.clone())),
                ),
            )
            .child(self.render_section_card(
                "Runtime",
                self.render_tunnel_runtime_card(runtime, live_deleted),
            ));

        let mut actions = div().flex().gap(px(8.0));
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

        panel.child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .pt_3()
                .border_t_1()
                .border_color(t.glass_border)
                .child(actions)
                .child(
                    primary_button("save tunnel", draft.can_save(), &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.save_tunnel_draft(cx);
                        }),
                    ),
                ),
        )
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
            let runtime_label = runtime
                .map(|snapshot| match snapshot.status {
                    PortForwardStatus::Starting => "starting",
                    PortForwardStatus::Running => "running",
                    PortForwardStatus::Failed => "failed",
                })
                .unwrap_or("stopped");
            rows = rows.child(
                self.render_list_row(
                    &tunnel.port_forward.label,
                    &format!(
                        "{}:{} -> {}:{}  {}",
                        tunnel.port_forward.listen_address,
                        tunnel.port_forward.listen_port,
                        tunnel.port_forward.target_address,
                        tunnel.port_forward.target_port,
                        runtime_label
                    ),
                    false,
                )
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
                    settings_action_chip(if runtime.is_some() { "stop" } else { "start" }, &t)
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
                .text_sm()
                .text_color(t.text_ghost)
                .child("Tunnel is stopped.");
        };

        let (status_label, status_color) = match runtime.status {
            PortForwardStatus::Starting => ("starting", t.warning),
            PortForwardStatus::Running => ("running", t.success),
            PortForwardStatus::Failed => ("failed", t.danger),
        };

        let mut card = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().w(px(8.0)).h(px(8.0)).rounded_full().bg(status_color))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.text_primary)
                            .child(status_label),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_color(t.text_muted)
                    .child(format!(
                        "connections: {}   in: {} B   out: {} B",
                        runtime.active_connections, runtime.bytes_in, runtime.bytes_out
                    )),
            );

        if live_deleted {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(t.warning)
                    .child("Saved rule deleted from vault; runtime remains active until stopped."),
            );
        }

        if let Some(error) = runtime.last_error.as_ref() {
            card = card.child(
                div()
                    .text_sm()
                    .text_color(t.danger)
                    .child(format!("error: {error}")),
            );
        }
        card
    }

    pub(crate) fn render_tunnel_monitor_section(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        if self.active_port_forwards.is_empty() {
            return div();
        }

        let frame = match (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            / 250)
            % 4
        {
            0 => "[=   ]",
            1 => "[==  ]",
            2 => "[=== ]",
            _ => "[ ===]",
        };

        let mut section =
            div()
                .flex()
                .flex_col()
                .gap(px(4.0))
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
            let status_color = match tunnel.status {
                PortForwardStatus::Starting => theme.warning,
                PortForwardStatus::Running => theme.accent,
                PortForwardStatus::Failed => theme.warning,
            };
            section = section.child(
                self.sidebar_row_shell(false)
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                            .text_color(status_color)
                            .child(frame),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(1.0))
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text_primary)
                                    .line_clamp(1)
                                    .child(if is_saved {
                                        format!("{mode} {}", tunnel.label)
                                    } else {
                                        format!("{mode} {} [deleted]", tunnel.label)
                                    }),
                            )
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                                    .text_color(if is_saved {
                                        theme.sidebar_meta
                                    } else {
                                        theme.warning
                                    })
                                    .line_clamp(1)
                                    .child(if is_saved {
                                        format!(
                                            "{}:{} -> {}:{}",
                                            tunnel.listen_address,
                                            tunnel.listen_port,
                                            tunnel.target_address,
                                            tunnel.target_port
                                        )
                                    } else {
                                        format!(
                                            "deleted from vault  {}:{} -> {}:{}",
                                            tunnel.listen_address,
                                            tunnel.listen_port,
                                            tunnel.target_address,
                                            tunnel.target_port
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                            .text_color(theme.warning)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.text_primary))
                            .child("stop")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.stop_saved_tunnel(&scope_key, cx);
                                }),
                            ),
                    ),
            );
        }

        section
    }
}
