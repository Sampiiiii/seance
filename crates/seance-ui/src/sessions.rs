// Owns session queries, lifecycle, SSH connection finalization, and session-specific UI.

use std::{collections::HashMap, sync::Arc};

use gpui::{Context, Div, FontWeight, MouseButton, Window, div, prelude::*, px};
use seance_core::SessionKind;
use seance_ssh::{SshConnectResult, SshError};
use seance_terminal::{TerminalGeometry, TerminalSession};

use crate::{
    SIDEBAR_FONT_MONO, SIDEBAR_MONO_SIZE_PX, SeanceWorkspace, forms::WorkspaceSurface,
    local_session_display_number_for_ids, perf::RedrawReason, refresh_app_menus,
    session_kind_map_from_sessions, ui_components::session_preview_text,
};

impl SeanceWorkspace {
    fn session_kind(&self, id: u64) -> Option<SessionKind> {
        self.backend.session_kind(id)
    }

    pub(crate) fn sessions(&self) -> Vec<Arc<dyn TerminalSession>> {
        self.backend.list_sessions()
    }

    fn local_session_display_number(&self, id: u64) -> Option<usize> {
        let sessions = self.sessions();
        let session_ids = sessions
            .iter()
            .map(|session| session.id())
            .collect::<Vec<_>>();
        let session_kinds = session_kind_map_from_sessions(&sessions, &self.backend);
        local_session_display_number_for_ids(&session_ids, &session_kinds, id)
    }

    fn session_display_title(&self, session: &Arc<dyn TerminalSession>) -> String {
        match self.session_kind(session.id()) {
            Some(SessionKind::Local) => self
                .local_session_display_number(session.id())
                .map(|number| format!("local-{number}"))
                .unwrap_or_else(|| session.title().to_string()),
            Some(SessionKind::Remote) | None => session.title().to_string(),
        }
    }

    fn session_display_badge(&self, session: &Arc<dyn TerminalSession>, active: bool) -> String {
        if active {
            return "live".into();
        }

        match self.session_kind(session.id()) {
            Some(SessionKind::Local) => self
                .local_session_display_number(session.id())
                .map(|number| format!("#{number}"))
                .unwrap_or_else(|| format!("#{}", session.id())),
            Some(SessionKind::Remote) | None => format!("#{}", session.id()),
        }
    }

    pub(crate) fn palette_session_labels(&self) -> HashMap<u64, String> {
        self.sessions()
            .iter()
            .map(|session| (session.id(), self.session_display_title(session)))
            .collect()
    }

    pub(crate) fn remote_session_ids(&self) -> Vec<u64> {
        self.sessions()
            .iter()
            .filter(|session| self.session_kind(session.id()) == Some(SessionKind::Remote))
            .map(|session| session.id())
            .collect()
    }

    pub(crate) fn schedule_session_watcher(
        window: &mut Window,
        cx: &mut Context<Self>,
        entity: gpui::Entity<Self>,
        notify_rx: std::sync::mpsc::Receiver<()>,
    ) {
        let notify_rx = Arc::new(std::sync::Mutex::new(notify_rx));
        window
            .spawn(cx, async move |cx| {
                loop {
                    let rx = Arc::clone(&notify_rx);
                    let recv_ok = cx
                        .background_executor()
                        .spawn(async move { rx.lock().unwrap().recv().is_ok() })
                        .await;
                    if !recv_ok {
                        break;
                    }
                    while notify_rx.lock().unwrap().try_recv().is_ok() {}
                    let _ = cx.update(|window, cx| {
                        entity.update(cx, |this, _| {
                            this.take_terminal_refresh_request();
                        });
                        window.refresh();
                    });
                }
            })
            .detach();
    }

    pub(crate) fn connect_saved_host(
        &mut self,
        vault_id: &str,
        host_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.connecting_host_id.is_some() {
            return;
        }

        let request = match self.backend.build_connect_request(vault_id, host_id) {
            Ok(request) => request,
            Err(err) => {
                self.show_toast(err.to_string());
                cx.notify();
                return;
            }
        };

        self.connecting_host_id = Some(crate::workspace::host_scope_key(vault_id, host_id));
        self.selected_host_id = Some(crate::workspace::host_scope_key(vault_id, host_id));
        self.show_toast("Connecting…");
        cx.notify();

        let ssh = self.backend.ssh_manager();
        let entity = cx.entity();

        window
            .spawn(cx, async move |cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { ssh.connect(request) })
                    .await;

                let _ = cx.update(|window, cx| {
                    entity.update(cx, |this, cx| {
                        this.finish_connect(result, window, cx);
                    });
                });
            })
            .detach();
    }

    fn finish_connect(
        &mut self,
        result: Result<SshConnectResult, SshError>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.connecting_host_id = None;
        match result {
            Ok(result) => {
                let session: Arc<dyn TerminalSession> = result.session;
                if let Some(geometry) = self.last_applied_geometry {
                    let _ = session.resize(geometry);
                }
                self.active_session_id = session.id();
                self.backend.register_remote_session(Arc::clone(&session));
                if let Some(notify_rx) = session.take_notify_rx() {
                    Self::schedule_session_watcher(window, cx, cx.entity(), notify_rx);
                }
                self.backend.touch_session(session.id());
                self.surface = WorkspaceSurface::Terminal;
                self.show_toast("SSH session connected.");
                self.invalidate_terminal_surface();
            }
            Err(err) => {
                self.show_toast(err.to_string());
            }
        }
        self.palette_open = false;
        self.perf_overlay.mark_input(RedrawReason::Palette);
        refresh_app_menus(cx);
        cx.notify();
    }

    pub(crate) fn active_session(&self) -> Option<Arc<dyn TerminalSession>> {
        self.backend.session(self.active_session_id)
    }

    pub(crate) fn spawn_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Ok(session) = self.backend.spawn_local_session() {
            if let Some(geometry) = self.last_applied_geometry {
                let _ = session.resize(geometry);
            }
            self.active_session_id = session.id();
            if let Some(notify_rx) = session.take_notify_rx() {
                Self::schedule_session_watcher(window, cx, cx.entity(), notify_rx);
            }
            self.backend.touch_session(session.id());
            self.invalidate_terminal_surface();
            self.perf_overlay.mark_input(RedrawReason::Input);
            refresh_app_menus(cx);
            cx.notify();
        }
    }

    pub(crate) fn select_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.active_session_id = id;
        self.backend.touch_session(id);
        if let Some(geometry) = self.last_applied_geometry
            && let Some(session) = self.active_session()
        {
            let _ = session.resize(geometry);
            self.active_terminal_rows = geometry.size.rows as usize;
        }
        self.invalidate_terminal_surface();
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    pub(crate) fn close_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.backend.close_session(id);
        if self
            .sftp_browser
            .as_ref()
            .is_some_and(|browser| browser.session_id() == id)
        {
            self.sftp_browser = None;
        }
        if self.active_session_id == id {
            self.active_session_id = self.backend.recent_session_id().unwrap_or(0);
        }
        if self.active_session_id == 0 {
            self.last_applied_geometry = None;
            self.active_terminal_rows = TerminalGeometry::default().size.rows as usize;
        }
        self.invalidate_terminal_surface();
        self.perf_overlay.mark_input(RedrawReason::Input);
        refresh_app_menus(cx);
        cx.notify();
    }

    fn render_session_row(
        &self,
        session: &Arc<dyn TerminalSession>,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = self.theme();
        let active = session.id() == self.active_session_id;
        let session_id = session.id();
        let title = self.session_display_title(session);
        let snapshot = session.snapshot();
        let has_output = snapshot
            .rows
            .iter()
            .any(|row| !row.plain_text().trim().is_empty());
        let preview = session_preview_text(&snapshot.rows).unwrap_or_else(|| {
            if has_output {
                "interactive session".into()
            } else {
                "waiting for output…".into()
            }
        });
        let close_session_id = session_id;
        let badge = self.session_display_badge(session, active);

        self.sidebar_row_shell(active)
            .child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(if active {
                        theme.accent
                    } else {
                        theme.text_ghost
                    })
                    .child(if active { ">" } else { " " }),
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
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(if active {
                                        theme.text_primary
                                    } else {
                                        theme.text_secondary
                                    })
                                    .line_clamp(1)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .font_family(SIDEBAR_FONT_MONO)
                                    .text_size(px(9.0))
                                    .px(px(7.0))
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .when(active, |el| {
                                        el.bg(theme.accent_glow).text_color(theme.accent)
                                    })
                                    .when(!active, |el| {
                                        el.bg(theme.glass_hover).text_color(theme.sidebar_meta)
                                    })
                                    .child(badge),
                            ),
                    )
                    .child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                            .text_color(theme.sidebar_meta)
                            .line_clamp(1)
                            .child(format!("$ {}", preview)),
                    ),
            )
            .child({
                let is_remote = self.session_kind(session_id) == Some(SessionKind::Remote);
                let sftp_session_id = session_id;
                let mut actions = div().flex().items_center().gap(px(4.0));
                if is_remote {
                    actions = actions.child(
                        div()
                            .font_family(SIDEBAR_FONT_MONO)
                            .text_size(px(SIDEBAR_MONO_SIZE_PX))
                            .text_color(theme.text_ghost)
                            .cursor_pointer()
                            .hover(|style| style.text_color(theme.accent))
                            .child("▤")
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, _, cx| {
                                    this.open_sftp_browser(sftp_session_id, cx);
                                }),
                            ),
                    );
                }
                actions = actions.child(
                    div()
                        .font_family(SIDEBAR_FONT_MONO)
                        .text_size(px(SIDEBAR_MONO_SIZE_PX))
                        .text_color(theme.text_ghost)
                        .cursor_pointer()
                        .hover(|style| style.text_color(theme.text_secondary))
                        .child("x")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.close_session(close_session_id, cx);
                            }),
                        ),
                );
                actions
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _, _, cx| {
                    this.select_session(session_id, cx);
                }),
            )
    }

    pub(crate) fn render_sessions_section(&self, cx: &mut Context<Self>) -> Div {
        let theme = self.theme();
        let sessions = self.sessions();
        let mut section = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .child(self.render_sidebar_section_heading("sessions", sessions.len().to_string()));

        if sessions.is_empty() {
            section = section.child(
                div()
                    .px(px(14.0))
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(theme.sidebar_meta)
                    .child("no active sessions"),
            );
        } else {
            let mut rows = div().flex().flex_col();
            for session in &sessions {
                rows = rows.child(self.render_session_row(session, cx));
            }
            section = section.child(rows);
        }

        section = section.child(
            div().px(px(14.0)).pt(px(2.0)).child(
                div()
                    .font_family(SIDEBAR_FONT_MONO)
                    .text_size(px(SIDEBAR_MONO_SIZE_PX))
                    .text_color(theme.text_ghost)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.text_secondary))
                    .child("+ new local session")
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.spawn_session(window, cx);
                        }),
                    ),
            ),
        );

        section
    }
}
