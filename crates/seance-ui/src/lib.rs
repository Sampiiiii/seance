#![allow(
    clippy::collapsible_if,
    clippy::items_after_test_module,
    clippy::too_many_arguments,
    clippy::type_complexity,
    clippy::unnecessary_map_or,
    clippy::unwrap_or_default
)]

mod actions;
mod app;
mod backend;
mod connect;
mod forms;
mod frame_pacer;
mod hosts;
mod keybindings;
mod model;
mod palette;
mod perf;
mod perf_runtime;
mod secure;
mod sessions;
mod settings;
mod sftp;
mod surface;
mod terminal_input;
mod terminal_links;
mod terminal_paint;
mod terminal_runtime;
mod terminal_scrollbar;
mod text_input;
mod theme;
mod tunnels;
mod ui_components;
mod vault;
mod workspace;
mod workspace_render;
mod workspace_scroll;

use std::time::Instant;

use gpui::{Context, MouseButton, Render, Window, deferred, div, prelude::*, px};
use seance_observability::{RenderDomain, RenderPath, RenderPhase, RenderTraceScope};

pub use actions::{
    CheckForUpdates, CloseActiveSession, ConnectHost, ConnectHostInNewWindow, HideOtherApps,
    HideSeance, NewTerminal, OpenCommandPalette, OpenNewWindow, OpenPreferences, QuitSeance,
    SelectNextSession, SelectPreviousSession, SelectSession, SelectSessionSlot, ShowAllApps,
    SwitchTheme, TogglePerfHud,
};
pub(crate) use app::refresh_app_menus;
pub use app::{UiCommand, UiIntegration, UiRuntime, run};
use forms::{SettingsSection, WorkspaceSurface};
pub(crate) use frame_pacer::{FramePacer, RepaintReasonSet};
use model::{
    MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH, SIDEBAR_DIVIDER_VISUAL_PX, SIDEBAR_DRAG_TARGET_PX,
};
pub(crate) use model::{
    SeanceWorkspace, TerminalMetrics, TerminalRendererMetrics,
    local_session_display_number_for_ids, session_kind_map_from_sessions,
};
pub(crate) use surface::{
    CachedRowPaintTemplate, CachedShapeLine, HslaKey, LinkPaintMode, PreparedTerminalSurface,
    RowPaintCache, RowPaintCacheKey, RowPaintTemplate, ShapeCache, ShapeCacheKey,
    TerminalFragmentPlan, TerminalGlyphPolicy, TerminalPaintFragment, TerminalPaintQuad,
    TerminalPaintRow,
};
pub(crate) use terminal_scrollbar::{
    TERMINAL_SCROLLBAR_GUTTER_WIDTH_PX, TerminalScrollbarDragState, TerminalScrollbarHit,
    TerminalScrollbarLayout,
};
pub(crate) use text_input::TextEditState;
pub use theme::ThemeId;

const SIDEBAR_FONT_MONO: &str = "JetBrains Mono";
const SIDEBAR_MONO_SIZE_PX: f32 = 11.0;

impl Render for SeanceWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let frame_trace = RenderTraceScope::new(
            RenderDomain::Ui,
            RenderPath::Frame,
            self.perf_overlay.pending_render_cause(),
        );
        let perf_enabled = self.perf_overlay.mode.is_enabled();
        let render_started_at = perf_enabled.then(Instant::now);

        if self.surface != WorkspaceSurface::Terminal && self.terminal_ime_visible() {
            self.clear_terminal_ime();
        }

        // Auto-dismiss toast after 3 seconds
        if let Some(toast) = &self.toast {
            if toast.shown_at.elapsed() >= std::time::Duration::from_secs(3) {
                self.toast = None;
            }
        }

        let t = self.theme();

        let main_content = {
            let _compose_phase = frame_trace.phase(RenderPhase::Compose);
            match self.surface {
                WorkspaceSurface::Terminal => self.render_terminal_shell(window, cx),
                WorkspaceSurface::Settings => self.render_settings_panel(window, cx),
                WorkspaceSurface::Sftp => self.render_sftp_panel(window, cx),
                WorkspaceSurface::Secure => self.render_secure_workspace(window, cx),
            }
        };

        let sidebar_resizing = self.sidebar_resizing;

        let drag_handle = div()
            .w(px(SIDEBAR_DRAG_TARGET_PX))
            .h_full()
            .flex()
            .justify_center()
            .flex_shrink_0()
            .cursor_col_resize()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.sidebar_resizing = true;
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(SIDEBAR_DIVIDER_VISUAL_PX))
                    .h_full()
                    .bg(if sidebar_resizing {
                        t.accent
                    } else {
                        t.sidebar_edge
                    }),
            );

        let mut root = div()
            .size_full()
            .flex()
            .bg(t.bg_deep)
            .text_color(t.text_primary)
            .on_mouse_move(
                cx.listener(|this, event: &gpui::MouseMoveEvent, window, _cx| {
                    if this.sidebar_resizing {
                        let new_width = f32::from(event.position.x);
                        this.sidebar_width = new_width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
                        this.schedule_active_terminal_geometry_refresh(window, _cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.sidebar_resizing {
                        this.sidebar_resizing = false;
                        cx.notify();
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &CheckForUpdates, _window, cx| {
                this.check_for_updates(cx);
            }))
            .on_action(cx.listener(|this, _: &OpenCommandPalette, _window, cx| {
                this.toggle_palette(cx);
            }))
            .on_action(cx.listener(|this, _: &NewTerminal, window, cx| {
                this.spawn_session(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenPreferences, _window, cx| {
                this.open_settings_panel(SettingsSection::General, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseActiveSession, window, cx| {
                if this.active_session_id != 0 {
                    this.close_session(this.active_session_id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &SelectPreviousSession, window, cx| {
                this.select_previous_session(window, cx);
            }))
            .on_action(cx.listener(|this, _: &SelectNextSession, window, cx| {
                this.select_next_session(window, cx);
            }))
            .on_action(cx.listener(|this, action: &SelectSessionSlot, window, cx| {
                this.select_session_slot(action.slot, window, cx);
            }))
            .on_action(cx.listener(|this, _: &TogglePerfHud, window, cx| {
                this.toggle_perf_mode(window, cx);
            }))
            .on_action(cx.listener(|this, action: &ConnectHost, window, cx| {
                this.selected_host_id = Some(crate::workspace::host_scope_key(
                    &action.vault_id,
                    &action.host_id,
                ));
                this.start_connect_attempt(&action.vault_id, &action.host_id, window, cx);
            }))
            .on_action(cx.listener(|this, action: &SelectSession, window, cx| {
                if this.backend.session(action.session_id).is_some() {
                    this.select_session(action.session_id, window, cx);
                }
            }))
            .on_action(cx.listener(|this, action: &SwitchTheme, window, cx| {
                this.persist_theme(action.theme_id, window, cx);
            }))
            .child(self.render_sidebar(cx))
            .child(drag_handle)
            .child(main_content);

        if self.palette_open {
            root = root.child(deferred(self.render_palette_overlay(window, cx)).with_priority(1));
        }
        if self.vault_modal.is_visible() {
            root = root.child(deferred(self.render_vault_modal(cx)).with_priority(3));
        }
        if self.confirm_dialog.is_some() {
            root = root.child(deferred(self.render_confirm_dialog(cx)).with_priority(5));
        }
        if self.perf_overlay.mode.is_enabled() {
            root = root.child(deferred(self.render_perf_overlay()).with_priority(4));
        }
        if self.toast.is_some() {
            root = root.child(deferred(self.render_toast()).with_priority(6));
        }

        if let Some(started_at) = render_started_at {
            self.perf_overlay.finish_render(started_at, Instant::now());
        }

        root
    }
}

pub(crate) fn now_ui_suffix() -> i64 {
    seance_vault::now_ts()
}
