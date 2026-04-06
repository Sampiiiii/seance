// Owns the settings panel UI plus settings-specific persistence and updater actions.

use gpui::{App, Context, Div, FontWeight, MouseButton, Window, div, prelude::*, px};
use seance_config::{PerfHudDefault, TerminalConfig, WindowConfig};
use seance_core::UpdateState;

use crate::{
    SeanceWorkspace,
    forms::SettingsSection,
    theme::ThemeId,
    ui_components::{
        perf_mode_label, settings_action_chip, settings_choice_chip, settings_info_card,
        settings_nav_button, settings_stepper_card, settings_toggle_card, update_status_label,
    },
};

impl SeanceWorkspace {
    pub(crate) fn is_settings_panel_open(&self) -> bool {
        self.settings_panel.open
    }

    pub(crate) fn open_settings_panel(&mut self, section: SettingsSection, cx: &mut Context<Self>) {
        if matches!(section, SettingsSection::Vault) && !self.vault_unlocked() {
            self.unlock_form.reset_for_unlock();
            self.unlock_form.message =
                Some("Unlock the vault to manage credentials and keys.".into());
            cx.notify();
            return;
        }
        if matches!(section, SettingsSection::Vault) {
            self.refresh_vault_cache();
        }
        self.settings_panel.open = true;
        self.settings_panel.section = section;
        self.settings_panel.message = None;
        self.palette_open = false;
        cx.notify();
    }

    pub(crate) fn close_settings_panel(&mut self, cx: &mut Context<Self>) {
        self.settings_panel.open = false;
        self.settings_panel.message = None;
        cx.notify();
    }

    pub(crate) fn persist_theme(
        &mut self,
        theme_id: ThemeId,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.backend.set_theme(theme_id.key().to_string()) {
            Ok(_) => {
                self.settings_panel.message = None;
                self.status_message = Some(format!("Theme set to {}.", theme_id.display_name()));
            }
            Err(error) => {
                self.settings_panel.message = Some(error.to_string());
                self.status_message = Some(error.to_string());
            }
        }
        self.perf_overlay
            .mark_input(crate::perf::RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn persist_window_settings(
        &mut self,
        window_settings: WindowConfig,
        cx: &mut Context<Self>,
    ) {
        match self.backend.set_window_settings(window_settings) {
            Ok(_) => self.settings_panel.message = None,
            Err(error) => self.settings_panel.message = Some(error.to_string()),
        }
        self.perf_overlay
            .mark_input(crate::perf::RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn persist_terminal_settings(
        &mut self,
        terminal_settings: TerminalConfig,
        cx: &mut Context<Self>,
    ) {
        match self.backend.set_terminal_settings(terminal_settings) {
            Ok(_) => self.settings_panel.message = None,
            Err(error) => self.settings_panel.message = Some(error.to_string()),
        }
        self.perf_overlay
            .mark_input(crate::perf::RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn persist_perf_hud_default(
        &mut self,
        perf_hud_default: PerfHudDefault,
        cx: &mut Context<Self>,
    ) {
        match self.backend.set_perf_hud_default(perf_hud_default) {
            Ok(_) => self.settings_panel.message = None,
            Err(error) => self.settings_panel.message = Some(error.to_string()),
        }
        self.perf_overlay
            .mark_input(crate::perf::RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn persist_update_auto_check(&mut self, auto_check: bool, cx: &mut Context<Self>) {
        match self.backend.controller().update_config(|config| {
            config.updates.auto_check = auto_check;
        }) {
            Ok(_) => self.settings_panel.message = None,
            Err(error) => self.settings_panel.message = Some(error.to_string()),
        }
        self.perf_overlay
            .mark_input(crate::perf::RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        self.backend.check_for_updates();
        self.status_message = Some("Checking for updates…".into());
        self.update_state = UpdateState::Checking;
        self.perf_overlay
            .mark_input(crate::perf::RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn install_available_update(&mut self, cx: &mut Context<Self>) {
        self.backend.install_update();
        self.perf_overlay
            .mark_input(crate::perf::RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn dismiss_update_banner(&mut self, cx: &mut Context<Self>) {
        self.backend.dismiss_update();
        self.update_state = UpdateState::Idle;
        self.perf_overlay
            .mark_input(crate::perf::RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn reset_settings_to_defaults(&mut self, cx: &mut Context<Self>) {
        match self.backend.reset_settings_to_defaults() {
            Ok(_) => self.settings_panel.message = Some("Settings reset to defaults.".into()),
            Err(error) => self.settings_panel.message = Some(error.to_string()),
        }
        self.perf_overlay
            .mark_input(crate::perf::RedrawReason::UiRefresh);
        cx.notify();
    }

    pub(crate) fn render_settings_panel(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = self.theme();
        let section = self.settings_panel.section;

        let shell_divider = div()
            .w(px(2.0))
            .h_full()
            .border_l_1()
            .border_color(t.sidebar_edge_bright)
            .bg(t.shell_divider_glow);

        let mut nav = div()
            .w(px(190.0))
            .h_full()
            .p_4()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .bg(t.sidebar_bg_elevated)
            .border_r_1()
            .border_color(t.sidebar_edge)
            .child(
                div()
                    .pb(px(10.0))
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(
                        div()
                            .text_size(px(18.0))
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text_primary)
                            .child("Preferences"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child("Live config backed by config.toml"),
                    ),
            );

        for item in SettingsSection::ALL {
            nav = nav.child(
                settings_nav_button(item, section == item, &t).on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.open_settings_panel(item, cx);
                    }),
                ),
            );
        }

        let mut content = div()
            .flex_1()
            .h_full()
            .bg(t.bg_void)
            .overflow_hidden()
            .track_focus(&self.focus_handle)
            .on_mouse_down(MouseButton::Left, {
                let fh = self.focus_handle.clone();
                move |_: &gpui::MouseDownEvent, window: &mut Window, _cx: &mut App| {
                    window.focus(&fh);
                }
            })
            .on_key_down(cx.listener(Self::handle_key_down))
            .p_6()
            .flex()
            .flex_col()
            .gap_6();

        let title = section.title();
        let subtitle = section.subtitle();
        content = content.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_size(px(20.0))
                                .font_weight(FontWeight::BOLD)
                                .text_color(t.text_primary)
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(12.0))
                                .text_color(t.text_muted)
                                .child(subtitle),
                        ),
                )
                .child(
                    div()
                        .px_3()
                        .py(px(6.0))
                        .rounded_md()
                        .bg(t.glass_tint)
                        .border_1()
                        .border_color(t.glass_border)
                        .text_xs()
                        .text_color(t.text_secondary)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.glass_hover).text_color(t.text_primary))
                        .child("esc  back to terminal")
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.close_settings_panel(cx);
                            }),
                        ),
                ),
        );

        if let Some(message) = self.settings_panel.message.clone() {
            content = content.child(
                div()
                    .px_4()
                    .py(px(10.0))
                    .rounded_lg()
                    .bg(t.glass_tint)
                    .border_1()
                    .border_color(t.glass_border)
                    .text_sm()
                    .text_color(t.text_secondary)
                    .child(message),
            );
        }

        content = match section {
            SettingsSection::General => {
                let window_settings = self.config.window;
                content
                    .child(
                        settings_toggle_card(
                            "Resident process",
                            "Keep Seance running after the last window closes.",
                            window_settings.keep_running_without_windows,
                            &t,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                let mut next = window_settings;
                                next.keep_running_without_windows =
                                    !next.keep_running_without_windows;
                                this.persist_window_settings(next, cx);
                            }),
                        ),
                    )
                    .child(
                        settings_toggle_card(
                            "Hide on last close",
                            "Hide the app instead of exiting when the last window closes.",
                            window_settings.hide_on_last_window_close,
                            &t,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                let mut next = window_settings;
                                next.hide_on_last_window_close =
                                    !next.hide_on_last_window_close;
                                this.persist_window_settings(next, cx);
                            }),
                        ),
                    )
                    .child(
                        settings_toggle_card(
                            "Keep sessions alive",
                            "Allow sessions to survive while the resident app stays open.",
                            window_settings.keep_sessions_alive_without_windows,
                            &t,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                let mut next = window_settings;
                                next.keep_sessions_alive_without_windows =
                                    !next.keep_sessions_alive_without_windows;
                                this.persist_window_settings(next, cx);
                            }),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .p_4()
                            .rounded_xl()
                            .bg(t.glass_tint)
                            .border_1()
                            .border_color(t.glass_border)
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.0))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(FontWeight::BOLD)
                                            .text_color(t.text_primary)
                                            .child("Keybindings"),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(t.text_muted)
                                            .child("Override schema is persisted, but runtime rebinding UI is still deferred."),
                                    ),
                            )
                            .child(
                                div()
                                    .px_3()
                                    .py(px(6.0))
                                    .rounded_md()
                                    .bg(t.accent_glow)
                                    .text_xs()
                                    .text_color(t.text_primary)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.accent))
                                    .child("reset defaults")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.reset_settings_to_defaults(cx);
                                        }),
                                    ),
                            ),
                    )
            }
            SettingsSection::Updates => {
                let updates = self.config.updates;
                let update_state = self.update_state.clone();
                let update_version = match &update_state {
                    UpdateState::Available(update) => update.version.clone(),
                    _ => "none".into(),
                };

                let mut actions = div().flex().flex_wrap().gap(px(8.0)).child(
                    settings_action_chip("check now", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.check_for_updates(cx);
                        }),
                    ),
                );

                if matches!(update_state, UpdateState::Available(_)) {
                    actions =
                        actions.child(settings_action_chip("install update", &t).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.install_available_update(cx);
                            }),
                        ));
                }

                if !matches!(update_state, UpdateState::Idle) {
                    actions = actions.child(settings_action_chip("dismiss", &t).on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.dismiss_update_banner(cx);
                        }),
                    ));
                }

                content
                    .child(
                        settings_toggle_card(
                            "Automatic checks",
                            "Check the stable release channel on app startup.",
                            updates.auto_check,
                            &t,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.persist_update_auto_check(!updates.auto_check, cx);
                            }),
                        ),
                    )
                    .child(
                        settings_info_card(
                            "Current build",
                            env!("CARGO_PKG_VERSION").to_string(),
                            "The updater compares this version against GitHub Releases.",
                            &t,
                        )
                        .child(
                            div()
                                .flex()
                                .flex_wrap()
                                .gap(px(8.0))
                                .child(settings_choice_chip("stable", true, &t))
                                .child(settings_choice_chip("prompted install", true, &t)),
                        ),
                    )
                    .child(
                        settings_info_card(
                            "Updater state",
                            update_status_label(&update_state).to_string(),
                            "Official builds support Sparkle on macOS and AppImage updating on Linux.",
                            &t,
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(6.0))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(t.text_muted)
                                        .child(format!("available version: {update_version}")),
                                )
                                .child(actions),
                        ),
                    )
            }
            SettingsSection::Appearance => {
                let themes = div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .text_color(t.text_primary)
                            .child("Bundled Themes"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(t.text_muted)
                            .child("Choose a theme and apply it live across all open windows."),
                    );

                let mut theme_grid = div().flex().flex_wrap().gap(px(8.0));
                for &theme_id in ThemeId::ALL {
                    let is_active = theme_id == self.active_theme;
                    let swatch = theme_id.theme().accent;
                    theme_grid = theme_grid.child(
                        settings_choice_chip(theme_id.display_name(), is_active, &t)
                            .child(div().w(px(9.0)).h(px(9.0)).rounded_full().bg(swatch))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, _, window, cx| {
                                    this.persist_theme(theme_id, window, cx);
                                }),
                            ),
                    );
                }

                content.child(
                    div()
                        .p_4()
                        .rounded_xl()
                        .bg(t.glass_tint)
                        .border_1()
                        .border_color(t.glass_border)
                        .child(themes.child(theme_grid)),
                )
            }
            SettingsSection::Terminal => {
                let terminal = self.config.terminal.clone();
                let shell_choices = [
                    ("default", None),
                    ("/bin/zsh", Some("/bin/zsh")),
                    ("/bin/bash", Some("/bin/bash")),
                    ("/bin/sh", Some("/bin/sh")),
                ];
                let font_choices = ["Menlo", "JetBrains Mono", "SF Mono", "Monaco"];

                let mut shell_row = div().flex().flex_wrap().gap(px(8.0));
                for (label, shell) in shell_choices {
                    let is_active = terminal.local_shell.as_deref() == shell;
                    shell_row =
                        shell_row.child(settings_choice_chip(label, is_active, &t).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                let mut next = this.config.terminal.clone();
                                next.local_shell = shell.map(str::to_string);
                                this.persist_terminal_settings(next, cx);
                            }),
                        ));
                }

                let mut font_row = div().flex().flex_wrap().gap(px(8.0));
                for font_family in font_choices {
                    let is_active = terminal.font_family == font_family;
                    font_row = font_row.child(
                        settings_choice_chip(font_family, is_active, &t).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                let mut next = this.config.terminal.clone();
                                next.font_family = font_family.to_string();
                                this.persist_terminal_settings(next, cx);
                            }),
                        ),
                    );
                }

                content
                    .child(
                        settings_info_card(
                            "Local shell",
                            terminal
                                .local_shell
                                .clone()
                                .unwrap_or_else(|| "default ($SHELL or /bin/bash)".into()),
                            "These defaults only affect newly created local sessions.",
                            &t,
                        )
                        .child(shell_row),
                    )
                    .child(
                        settings_info_card(
                            "Font family",
                            terminal.font_family.clone(),
                            "Preset terminal font families.",
                            &t,
                        )
                        .child(font_row),
                    )
                    .child(
                        settings_stepper_card(
                            "Font size",
                            format!("{:.1}px", terminal.font_size_px),
                            "Resize the terminal text rendering baseline.",
                            &t,
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .child(settings_action_chip("-0.5", &t).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        let mut next = this.config.terminal.clone();
                                        next.font_size_px = (next.font_size_px - 0.5).max(8.0);
                                        this.persist_terminal_settings(next, cx);
                                    }),
                                ))
                                .child(settings_action_chip("+0.5", &t).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        let mut next = this.config.terminal.clone();
                                        next.font_size_px = (next.font_size_px + 0.5).min(32.0);
                                        this.persist_terminal_settings(next, cx);
                                    }),
                                )),
                        ),
                    )
                    .child(
                        settings_stepper_card(
                            "Line height",
                            format!("{:.1}px", terminal.line_height_px),
                            "Controls row spacing and terminal geometry.",
                            &t,
                        )
                        .child(
                            div()
                                .flex()
                                .gap(px(8.0))
                                .child(settings_action_chip("-0.5", &t).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        let mut next = this.config.terminal.clone();
                                        next.line_height_px =
                                            (next.line_height_px - 0.5).max(10.0);
                                        this.persist_terminal_settings(next, cx);
                                    }),
                                ))
                                .child(settings_action_chip("+0.5", &t).on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|this, _, _, cx| {
                                        let mut next = this.config.terminal.clone();
                                        next.line_height_px =
                                            (next.line_height_px + 0.5).min(40.0);
                                        this.persist_terminal_settings(next, cx);
                                    }),
                                )),
                        ),
                    )
            }
            SettingsSection::Debug => {
                let mut choices = div().flex().gap(px(8.0));
                for (label, mode) in [
                    ("off", PerfHudDefault::Off),
                    ("compact", PerfHudDefault::Compact),
                    ("expanded", PerfHudDefault::Expanded),
                ] {
                    let is_active = self.config.debug.perf_hud_default == mode;
                    choices =
                        choices.child(settings_choice_chip(label, is_active, &t).on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.persist_perf_hud_default(mode, cx);
                            }),
                        ));
                }

                content.child(
                    settings_info_card(
                        "Performance HUD",
                        perf_mode_label(self.config.debug.perf_hud_default.into()).to_string(),
                        "The SEANCE_PERF_HUD env var still overrides this for the current process.",
                        &t,
                    )
                    .child(choices),
                )
            }
            SettingsSection::Vault => content
                .child(self.render_vault_credentials_card(cx))
                .child(self.render_vault_keys_card(cx)),
        };

        div()
            .flex_1()
            .h_full()
            .flex()
            .child(nav)
            .child(shell_divider)
            .child(content)
    }
}