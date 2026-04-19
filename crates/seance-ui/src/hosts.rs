// Owns the sidebar host list and host-list refresh helpers.

use gpui::{Context, Div, FontWeight, MouseButton, div, prelude::*, px};

use crate::{
    ConnectHostInNewWindow, SIDEBAR_FONT_MONO, SIDEBAR_MONO_SIZE_PX, SeanceWorkspace,
    forms::{UnlockMode, VaultModalOrigin},
    perf::RedrawReason,
    workspace::host_scope_key,
};

impl SeanceWorkspace {
    pub(crate) fn refresh_saved_hosts(&mut self) {
        self.refresh_managed_vaults();
        self.saved_hosts = self.backend.list_hosts().unwrap_or_default();

        if self.selected_host_id.as_ref().is_some_and(|id| {
            !self
                .saved_hosts
                .iter()
                .any(|host| host_scope_key(&host.vault_id, &host.host.id) == *id)
        }) {
            self.selected_host_id = self
                .saved_hosts
                .first()
                .map(|host| host_scope_key(&host.vault_id, &host.host.id));
        }
        self.rebuild_secure_search_cache();
    }

    fn render_host_row(
        &self,
        host: &seance_core::VaultScopedHostSummary,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme();
        let scope_key = host_scope_key(&host.vault_id, &host.host.id);
        let selected = self
            .selected_host_id
            .as_ref()
            .is_some_and(|id| id == &scope_key);
        let pending_attempt_id = self.connect_attempts.attempt_id_for_host(&scope_key);
        let is_connecting = pending_attempt_id.is_some();
        let host_id = host.host.id.clone();
        let vault_id = host.vault_id.clone();
        let label = host.host.label.clone();
        let target = format!(
            "{}@{}:{}",
            host.host.username, host.host.hostname, host.host.port
        );
        let vault_label = host.vault_name.clone();
        let mut meta = div().flex().items_center().gap(px(8.0)).child(
            div()
                .font_family(SIDEBAR_FONT_MONO)
                .text_size(px(SIDEBAR_MONO_SIZE_PX))
                .text_color(theme.text_ghost)
                .child(vault_label),
        );
        if let Some(attempt_id) = pending_attempt_id {
            meta = meta.child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(theme.warning)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.text_primary))
                    .child("cancel")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            this.cancel_connect_attempt(attempt_id, cx);
                        }),
                    ),
            );
        }

        self.sidebar_row_shell(selected || is_connecting)
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(if selected || is_connecting {
                        theme.accent
                    } else {
                        theme.text_ghost
                    })
                    .child(if is_connecting {
                        "\u{2022}"
                    } else if selected {
                        ">"
                    } else {
                        " "
                    }),
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
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(if selected || is_connecting {
                                theme.text_primary
                            } else {
                                theme.text_secondary
                            })
                            .line_clamp(1)
                            .child(label),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                            .text_color(theme.sidebar_meta)
                            .line_clamp(1)
                            .child(target),
                    ),
            )
            .child(meta)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    this.selected_host_id = Some(scope_key.clone());
                    if event.modifiers.shift {
                        cx.dispatch_action(&ConnectHostInNewWindow {
                            vault_id: vault_id.clone(),
                            host_id: host_id.clone(),
                        });
                        return;
                    }
                    if is_connecting {
                        cx.notify();
                        return;
                    }
                    this.start_connect_attempt(&vault_id, &host_id, window, cx);
                }),
            )
    }

    pub(crate) fn render_hosts_section(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let unlocked = self.vault_unlocked();
        let meta = if unlocked {
            self.saved_hosts.len().to_string()
        } else {
            "locked".into()
        };

        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(self.render_sidebar_section_heading("hosts", meta));

        if unlocked {
            if self.saved_hosts.is_empty() {
                section = section.child(
                    div()
                        .px(px(14.0))
                        .font_family(SIDEBAR_FONT_MONO)
                        .text_size(px(SIDEBAR_MONO_SIZE_PX))
                        .text_color(theme.sidebar_meta)
                        .child("no saved hosts"),
                );
            } else {
                let mut rows = div().flex().flex_col();
                for host in &self.saved_hosts {
                    rows = rows.child(self.render_host_row(host, cx));
                }
                section = section.child(rows);
            }

            section = section.child(
                div()
                    .px(px(14.0))
                    .pt(px(2.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                            .text_color(theme.text_ghost)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.text_secondary))
                            .child("+ add host")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.begin_add_host(cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                            .text_color(theme.text_ghost)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.text_secondary))
                            .child("+ host wizard")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.begin_new_host_wizard(cx);
                                }),
                            ),
                    ),
            );
        } else {
            section = section.child(
                div()
                    .px(px(14.0))
                    .py(px(6.0))
                    .cursor_pointer()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(theme.text_muted)
                    .hover(|style| style.text_color(theme.text_secondary))
                    .child("vault locked -- unlock to view")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.open_vault_modal(
                                UnlockMode::Unlock,
                                VaultModalOrigin::SecureAccess,
                                "Enter the recovery passphrase to unlock the vault.".into(),
                                cx,
                            );
                            this.perf_overlay.mark_input(RedrawReason::Input);
                        }),
                    ),
            );
        }

        section
    }
}
