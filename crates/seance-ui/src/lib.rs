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
mod forms;
mod hosts;
mod model;
mod palette;
mod perf;
mod settings;
mod sftp;
mod sessions;
mod surface;
mod terminal_paint;
mod theme;
mod ui_components;
mod vault;
mod workspace;
mod workspace_render;

use std::time::Instant;

use gpui::{Context, Render, Window, deferred, div, prelude::*};

pub use actions::{
    CheckForUpdates, CloseActiveSession, ConnectHost, HideOtherApps, HideSeance, NewTerminal,
    OpenCommandPalette, OpenNewWindow, OpenPreferences, QuitSeance, SelectSession, ShowAllApps,
    SwitchTheme, TogglePerfHud,
};
pub use app::{UiCommand, UiIntegration, UiRuntime, run};
use forms::SettingsSection;
pub(crate) use app::refresh_app_menus;
pub(crate) use model::{
    SeanceWorkspace, TerminalMetrics, TerminalRendererMetrics, local_session_display_number_for_ids,
    session_kind_map_from_sessions,
};
pub(crate) use surface::{
    CachedShapeLine, HslaKey, PreparedTerminalSurface, ShapeCache, ShapeCacheKey,
    TerminalFragmentPlan, TerminalGlyphPolicy, TerminalPaintFragment, TerminalPaintQuad, TerminalPaintRow,
};
pub use theme::ThemeId;

const SIDEBAR_FONT_MONO: &str = "JetBrains Mono";
const SIDEBAR_MONO_SIZE_PX: f32 = 11.0;

impl Render for SeanceWorkspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let perf_enabled = self.perf_overlay.mode.is_enabled();
        let render_started_at = perf_enabled.then(Instant::now);

        let t = self.theme();

        let main_content = if self.is_settings_panel_open() {
            self.render_settings_panel(window, cx)
        } else if self.sftp_browser.is_some() {
            self.render_sftp_panel(window, cx)
        } else {
            self.render_terminal_shell(window, cx)
        };

        let mut root = div()
            .size_full()
            .flex()
            .bg(t.bg_deep)
            .text_color(t.text_primary)
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
            .on_action(cx.listener(|this, _: &CloseActiveSession, _window, cx| {
                if this.active_session_id != 0 {
                    this.close_session(this.active_session_id, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &TogglePerfHud, window, cx| {
                this.toggle_perf_mode(window, cx);
            }))
            .on_action(cx.listener(|this, action: &ConnectHost, window, cx| {
                this.selected_host_id = Some(action.host_id.clone());
                this.connect_saved_host(&action.host_id, window, cx);
            }))
            .on_action(cx.listener(|this, action: &SelectSession, _window, cx| {
                if this.backend.session(action.session_id).is_some() {
                    this.select_session(action.session_id, cx);
                }
            }))
            .on_action(cx.listener(|this, action: &SwitchTheme, window, cx| {
                this.persist_theme(action.theme_id, window, cx);
            }))
            .child(self.render_sidebar(cx))
            .child(main_content);

        if self.palette_open {
            root = root.child(deferred(self.render_palette_overlay(cx)).with_priority(1));
        }
        if self.host_editor.is_some() {
            root = root.child(deferred(self.render_host_editor_overlay(cx)).with_priority(2));
        }
        if self.credential_editor.is_some() {
            root = root.child(deferred(self.render_credential_editor_overlay()).with_priority(5));
        }
        if self.unlock_form.is_visible() {
            root = root.child(deferred(self.render_unlock_overlay()).with_priority(3));
        }
        if self.perf_overlay.mode.is_enabled() {
            root = root.child(deferred(self.render_perf_overlay()).with_priority(4));
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
