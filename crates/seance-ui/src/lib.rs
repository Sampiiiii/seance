mod palette;
mod theme;

use std::time::Duration;

use anyhow::Result;
use gpui::{
    App, Application, Bounds, Context, FocusHandle, Focusable, FontWeight, KeyDownEvent,
    MouseButton, SharedString, StyledText, TextRun, UnderlineStyle, Window,
    WindowBackgroundAppearance, WindowBounds, WindowOptions, deferred, div, font, prelude::*, px,
    size,
};
use seance_terminal::{
    LocalSessionFactory, LocalSessionHandle, TerminalCellStyle, TerminalColor, TerminalLine,
};

use palette::{PaletteAction, build_items};
use theme::{Theme, ThemeId};

const TERMINAL_LINE_COUNT: usize = 40;
const SIDEBAR_WIDTH: f32 = 260.0;

pub fn run() -> Result<()> {
    Application::new().run(|cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_background: WindowBackgroundAppearance::Blurred,
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("Séance".into()),
                    appears_transparent: true,
                    ..Default::default()
                }),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let focus_handle = cx.focus_handle();
                    focus_handle.focus(window);

                    let factory = LocalSessionFactory::default();
                    let initial = factory
                        .spawn()
                        .expect("failed to create initial local session");

                    let ws = SeanceWorkspace {
                        focus_handle,
                        session_factory: factory,
                        sessions: vec![initial],
                        active_session_id: 1,
                        active_theme: ThemeId::ObsidianSmoke,
                        palette_open: false,
                        palette_query: String::new(),
                        palette_selected: 0,
                    };
                    ws.schedule_refresh(window, cx);
                    ws
                })
            },
        )
        .expect("failed to open Séance window");
    });
    Ok(())
}

struct SeanceWorkspace {
    focus_handle: FocusHandle,
    session_factory: LocalSessionFactory,
    sessions: Vec<LocalSessionHandle>,
    active_session_id: u64,
    active_theme: ThemeId,
    palette_open: bool,
    palette_query: String,
    palette_selected: usize,
}

impl SeanceWorkspace {
    fn theme(&self) -> Theme {
        self.active_theme.theme()
    }

    fn schedule_refresh(&self, window: &mut Window, cx: &mut Context<Self>) {
        window
            .spawn(cx, async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(33))
                        .await;
                    let _ = cx.update(|window, _cx| {
                        window.refresh();
                    });
                }
            })
            .detach();
    }

    fn active_session(&self) -> Option<&LocalSessionHandle> {
        self.sessions
            .iter()
            .find(|s| s.id() == self.active_session_id)
    }

    fn spawn_session(&mut self, cx: &mut Context<Self>) {
        if let Ok(session) = self.session_factory.spawn() {
            self.active_session_id = session.id();
            self.sessions.push(session);
            cx.notify();
        }
    }

    fn select_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.active_session_id = id;
        cx.notify();
    }

    fn close_session(&mut self, id: u64, cx: &mut Context<Self>) {
        self.sessions.retain(|s| s.id() != id);
        if self.active_session_id == id {
            self.active_session_id = self.sessions.last().map(|s| s.id()).unwrap_or(0);
        }
        cx.notify();
    }

    fn toggle_palette(&mut self, cx: &mut Context<Self>) {
        if self.palette_open {
            self.palette_open = false;
        } else {
            self.palette_open = true;
            self.palette_query.clear();
            self.palette_selected = 0;
        }
        cx.notify();
    }

    fn execute_palette_action(&mut self, action: PaletteAction, cx: &mut Context<Self>) {
        match action {
            PaletteAction::NewLocalTerminal => self.spawn_session(cx),
            PaletteAction::SwitchSession(id) => self.select_session(id, cx),
            PaletteAction::CloseActiveSession => {
                let id = self.active_session_id;
                self.close_session(id, cx);
            }
            PaletteAction::SwitchTheme(tid) => {
                self.active_theme = tid;
            }
        }
        self.palette_open = false;
        self.palette_query.clear();
        self.palette_selected = 0;
        cx.notify();
    }

    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;

        if mods.platform && key == "k" {
            self.toggle_palette(cx);
            return;
        }
        if mods.platform && key == "t" {
            self.spawn_session(cx);
            return;
        }
        if mods.platform && key == "w" {
            if self.active_session_id != 0 {
                let id = self.active_session_id;
                self.close_session(id, cx);
            }
            return;
        }

        if self.palette_open {
            self.handle_palette_key(event, cx);
            return;
        }

        if let Some(bytes) = encode_keystroke(event)
            && let Some(session) = self.active_session()
        {
            let _ = session.send_input(bytes);
        }
        cx.notify();
    }

    fn handle_palette_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let key_char = event.keystroke.key_char.as_deref();

        match key {
            "escape" => {
                self.palette_open = false;
                self.palette_query.clear();
                self.palette_selected = 0;
                cx.notify();
            }
            "up" => {
                self.palette_selected = self.palette_selected.saturating_sub(1);
                cx.notify();
            }
            "down" => {
                let count = build_items(
                    &self.sessions,
                    self.active_session_id,
                    self.active_theme,
                    &self.palette_query,
                )
                .len();
                if self.palette_selected + 1 < count {
                    self.palette_selected += 1;
                }
                cx.notify();
            }
            "enter" => {
                let items = build_items(
                    &self.sessions,
                    self.active_session_id,
                    self.active_theme,
                    &self.palette_query,
                );
                if let Some(item) = items.get(self.palette_selected) {
                    let action = item.action.clone();
                    self.execute_palette_action(action, cx);
                }
            }
            "backspace" => {
                self.palette_query.pop();
                self.palette_selected = 0;
                cx.notify();
            }
            "tab" | "left" | "right" | "home" | "end" | "pageup" | "pagedown" => {}
            _ => {
                if let Some(ch) = key_char {
                    let m = event.keystroke.modifiers;
                    if !m.platform && !m.control && !m.function {
                        self.palette_query.push_str(ch);
                        self.palette_selected = 0;
                        cx.notify();
                    }
                }
            }
        }
    }

    // ─── Rendering ──────────────────────────────────────────

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();

        let mut session_list = div().flex().flex_col().gap_1().px_2();

        for session in &self.sessions {
            let active = session.id() == self.active_session_id;
            let sid = session.id();
            let title = session.title().to_string();
            let snapshot = session.snapshot();
            let preview = snapshot
                .lines
                .iter()
                .rev()
                .map(TerminalLine::plain_text)
                .find(|l| !l.trim().is_empty())
                .unwrap_or_else(|| "waiting for output…".into());

            let mut card = div()
                .px_3()
                .py_2()
                .rounded_lg()
                .cursor_pointer()
                .flex()
                .flex_col()
                .gap(px(2.0));

            card = if active {
                card.bg(t.glass_active)
                    .border_1()
                    .border_color(t.accent_glow)
            } else {
                card.hover(|s| s.bg(t.glass_hover))
            };

            let close_sid = sid;
            card = card
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().w(px(6.0)).h(px(6.0)).rounded_full().bg(if active {
                                    t.accent
                                } else {
                                    t.text_ghost
                                }))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::BOLD)
                                        .text_color(if active {
                                            t.text_primary
                                        } else {
                                            t.text_secondary
                                        })
                                        .child(title),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.text_ghost)
                                .cursor_pointer()
                                .hover(|s| s.text_color(t.text_secondary))
                                .child("×")
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, _, cx| {
                                        this.close_session(close_sid, cx);
                                    }),
                                ),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(t.text_muted)
                        .line_clamp(1)
                        .child(preview),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.select_session(sid, cx);
                    }),
                );

            session_list = session_list.child(card);
        }

        div()
            .w(px(SIDEBAR_WIDTH))
            .h_full()
            .flex()
            .flex_col()
            .justify_between()
            .bg(t.glass_tint)
            .border_r_1()
            .border_color(t.glass_border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(
                        div()
                            .pt(px(38.0))
                            .px_4()
                            .pb_2()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_size(px(18.0)).text_color(t.accent).child("◈"))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(t.text_primary)
                                    .child("Séance"),
                            ),
                    )
                    .child(
                        div()
                            .px_3()
                            .flex()
                            .items_center()
                            .justify_between()
                            .child(
                                div()
                                    .text_xs()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(t.text_muted)
                                    .child("SESSIONS"),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py(px(2.0))
                                    .rounded_md()
                                    .text_xs()
                                    .text_color(t.text_ghost)
                                    .cursor_pointer()
                                    .hover(|s| s.bg(t.glass_hover).text_color(t.text_muted))
                                    .child("+ new")
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.spawn_session(cx);
                                        }),
                                    ),
                            ),
                    )
                    .child(session_list),
            )
            .child(
                div()
                    .px_3()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .px_2()
                            .py(px(6.0))
                            .rounded_md()
                            .border_1()
                            .border_color(t.glass_border)
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .hover(|s| s.bg(t.glass_hover))
                            .child(div().text_xs().text_color(t.text_ghost).child("⌘K"))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(t.text_muted)
                                    .child("Command Palette"),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.toggle_palette(cx);
                                }),
                            ),
                    )
                    .child(
                        div()
                            .px_2()
                            .py(px(4.0))
                            .rounded_md()
                            .flex()
                            .items_center()
                            .gap_2()
                            .cursor_pointer()
                            .hover(|s| s.bg(t.glass_hover))
                            .child(div().text_xs().text_color(t.accent).child("◑"))
                            .child(div().text_xs().text_color(t.text_muted).child(t.name))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.palette_open = true;
                                    this.palette_query = "theme".into();
                                    this.palette_selected = 0;
                                    cx.notify();
                                }),
                            ),
                    ),
            )
    }

    fn render_terminal_pane(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();

        let base = div()
            .flex_1()
            .h_full()
            .bg(t.bg_void)
            .track_focus(&self.focus_handle)
            .on_mouse_down(MouseButton::Left, {
                let fh = self.focus_handle.clone();
                move |_: &gpui::MouseDownEvent, window: &mut Window, _cx: &mut App| {
                    window.focus(&fh);
                }
            })
            .on_key_down(cx.listener(Self::handle_key_down));

        if self.sessions.is_empty() || self.active_session().is_none() {
            return base
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_4()
                .child(
                    div()
                        .text_size(px(48.0))
                        .text_color(t.text_ghost)
                        .child("◈"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(t.text_muted)
                        .child("No active sessions"),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .border_1()
                                .border_color(t.glass_border)
                                .bg(t.glass_tint)
                                .text_xs()
                                .text_color(t.text_secondary)
                                .child("⌘K"),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.text_muted)
                                .child("to open command palette"),
                        ),
                );
        }

        let session = self.active_session().unwrap();
        let snapshot = session.snapshot();
        let mut visible_lines = snapshot.lines;
        if visible_lines.len() > TERMINAL_LINE_COUNT {
            let start = visible_lines.len() - TERMINAL_LINE_COUNT;
            visible_lines.drain(0..start);
        }

        let mut term = base
            .p_4()
            .font_family("Menlo")
            .text_size(px(13.0))
            .line_height(px(19.0))
            .text_color(t.text_primary);

        for line in visible_lines {
            term = term.child(render_terminal_line(&line, &t));
        }

        if let Some(exit_status) = snapshot.exit_status {
            term = term.child(
                div()
                    .mt_3()
                    .text_xs()
                    .text_color(t.warning)
                    .child(format!("[process exited: {exit_status}]")),
            );
        }

        term
    }

    fn render_palette_overlay(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();
        let items = build_items(
            &self.sessions,
            self.active_session_id,
            self.active_theme,
            &self.palette_query,
        );
        let selected = self.palette_selected.min(items.len().saturating_sub(1));

        let mut item_list = div().flex().flex_col().p_2();

        if items.is_empty() {
            item_list = item_list.child(
                div()
                    .py_3()
                    .flex()
                    .justify_center()
                    .text_sm()
                    .text_color(t.text_muted)
                    .child("No matching commands"),
            );
        }

        for (idx, item) in items.iter().enumerate() {
            let is_sel = idx == selected;
            let action = item.action.clone();

            let mut row = div()
                .px_3()
                .py(px(8.0))
                .rounded_lg()
                .flex()
                .items_center()
                .gap_3()
                .cursor_pointer();

            row = if is_sel {
                row.bg(t.selection_soft)
            } else {
                row.hover(|s| s.bg(t.glass_hover))
            };

            row = row
                .child(
                    div()
                        .w(px(24.0))
                        .flex()
                        .justify_center()
                        .text_sm()
                        .font_weight(FontWeight::BOLD)
                        .text_color(if is_sel { t.accent } else { t.text_muted })
                        .child(item.glyph),
                )
                .child(
                    div()
                        .flex_1()
                        .child(
                            div()
                                .text_sm()
                                .text_color(if is_sel {
                                    t.text_primary
                                } else {
                                    t.text_secondary
                                })
                                .child(item.label.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(t.text_muted)
                                .child(item.hint.clone()),
                        ),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.execute_palette_action(action.clone(), cx);
                    }),
                );

            item_list = item_list.child(row);
        }

        let panel = div()
            .w(px(540.0))
            .bg(t.glass_strong)
            .border_1()
            .border_color(t.glass_border_bright)
            .rounded_xl()
            .shadow_lg()
            .overflow_hidden()
            .flex()
            .flex_col()
            .child(
                div()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(t.glass_border)
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .text_sm()
                            .text_color(t.accent)
                            .font_weight(FontWeight::BOLD)
                            .child("›"),
                    )
                    .child(div().flex_1().flex().items_center().child(
                        if self.palette_query.is_empty() {
                            div()
                                .text_sm()
                                .text_color(t.text_muted)
                                .child("Search commands…")
                        } else {
                            div()
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(t.text_primary)
                                        .child(self.palette_query.clone()),
                                )
                                .child(div().w(px(2.0)).h(px(16.0)).ml(px(1.0)).bg(t.accent))
                        },
                    ))
                    .child(
                        div()
                            .px(px(6.0))
                            .py(px(2.0))
                            .rounded_md()
                            .border_1()
                            .border_color(t.glass_border)
                            .text_xs()
                            .text_color(t.text_ghost)
                            .child("esc"),
                    ),
            )
            .child(item_list)
            .child(
                div()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(t.glass_border)
                    .flex()
                    .items_center()
                    .gap_4()
                    .text_xs()
                    .text_color(t.text_ghost)
                    .child("↑↓ navigate")
                    .child("↵ select")
                    .child("esc close"),
            );

        div()
            .absolute()
            .size_full()
            .bg(t.scrim)
            .flex()
            .flex_col()
            .items_center()
            .pt(px(100.0))
            .child(panel)
    }
}

fn render_terminal_line(line: &TerminalLine, theme: &Theme) -> StyledText {
    let (text, runs) = build_text_runs(line, theme);
    StyledText::new(text).with_runs(runs)
}

fn build_text_runs(line: &TerminalLine, theme: &Theme) -> (SharedString, Vec<TextRun>) {
    if line.spans.is_empty() {
        let text: SharedString = " ".into();
        let runs = vec![text_run("Menlo", " ", TerminalCellStyle::default(), theme)];
        return (text, runs);
    }

    let mut text = String::new();
    let mut runs = Vec::with_capacity(line.spans.len());

    for span in &line.spans {
        text.push_str(&span.text);
        runs.push(text_run("Menlo", &span.text, span.style, theme));
    }

    (text.into(), runs)
}

fn text_run(family: &'static str, text: &str, style: TerminalCellStyle, theme: &Theme) -> TextRun {
    let mut terminal_font = font(family);
    if style.bold {
        terminal_font = terminal_font.bold();
    }
    if style.italic {
        terminal_font = terminal_font.italic();
    }

    TextRun {
        len: text.len(),
        font: terminal_font,
        color: resolve_terminal_foreground(style, theme),
        background_color: style.background.map(terminal_color_to_hsla),
        underline: style.underline.then_some(UnderlineStyle {
            thickness: px(1.0),
            color: Some(resolve_terminal_foreground(style, theme)),
            wavy: false,
        }),
        strikethrough: None,
    }
}

fn resolve_terminal_foreground(style: TerminalCellStyle, theme: &Theme) -> gpui::Hsla {
    let base = style
        .foreground
        .map(terminal_color_to_hsla)
        .unwrap_or(theme.text_primary);

    if !style.faint {
        return base;
    }

    soften_faint_terminal_foreground(base, theme)
}

fn soften_faint_terminal_foreground(base: gpui::Hsla, theme: &Theme) -> gpui::Hsla {
    let subdued = base.blend(theme.bg_void.alpha(0.62)).alpha(0.78);
    let cap = if lightness_distance(theme.text_ghost, theme.bg_void) >= 0.10 {
        theme.text_ghost
    } else {
        theme.text_muted
    };
    let subdued = if lightness_distance(subdued, theme.bg_void) < 0.10 {
        cap
    } else {
        subdued
    };

    if lightness_distance(subdued, theme.bg_void) > lightness_distance(cap, theme.bg_void) {
        subdued.blend(cap.alpha(0.55))
    } else {
        subdued
    }
}

fn lightness_distance(left: gpui::Hsla, right: gpui::Hsla) -> f32 {
    (left.l - right.l).abs()
}

fn terminal_color_to_hsla(color: TerminalColor) -> gpui::Hsla {
    gpui::rgb((u32::from(color.r) << 16) | (u32::from(color.g) << 8) | u32::from(color.b)).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{FontStyle, FontWeight};
    use seance_terminal::{TerminalLine, TerminalSpan};

    #[test]
    fn builds_runs_with_utf8_byte_lengths() {
        let line = TerminalLine {
            spans: vec![
                TerminalSpan {
                    text: "café".into(),
                    style: TerminalCellStyle::default(),
                },
                TerminalSpan {
                    text: " 👋".into(),
                    style: TerminalCellStyle {
                        bold: true,
                        ..TerminalCellStyle::default()
                    },
                },
            ],
        };

        let (text, runs) = build_text_runs(&line, &ThemeId::ObsidianSmoke.theme());

        assert_eq!(text.as_ref(), "café 👋");
        assert_eq!(runs.iter().map(|run| run.len).sum::<usize>(), text.len());
        assert_eq!(runs[0].len, "café".len());
        assert_eq!(runs[1].len, " 👋".len());
        assert_eq!(runs[1].font.weight, FontWeight::BOLD);
    }

    #[test]
    fn maps_background_and_underline_styles() {
        let line = TerminalLine {
            spans: vec![TerminalSpan {
                text: "styled".into(),
                style: TerminalCellStyle {
                    foreground: Some(TerminalColor { r: 255, g: 0, b: 0 }),
                    background: Some(TerminalColor { r: 0, g: 0, b: 0 }),
                    bold: false,
                    italic: true,
                    underline: true,
                    ..TerminalCellStyle::default()
                },
            }],
        };

        let (_text, runs) = build_text_runs(&line, &ThemeId::ObsidianSmoke.theme());

        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].font.style, FontStyle::Italic);
        assert!(runs[0].background_color.is_some());
        assert!(runs[0].underline.is_some());
    }

    #[test]
    fn faint_text_is_softened_for_ghost_text_rendering() {
        let theme = ThemeId::Bone.theme();
        let base = gpui::rgb(0x1a1816).into();

        let softened = soften_faint_terminal_foreground(base, &theme);

        assert!(lightness_distance(softened, theme.bg_void) >= 0.10);
        assert!(
            lightness_distance(softened, theme.bg_void)
                <= lightness_distance(theme.text_muted, theme.bg_void)
        );
    }
}

impl Focusable for SeanceWorkspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SeanceWorkspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme();

        let root = div()
            .size_full()
            .flex()
            .bg(t.bg_deep)
            .text_color(t.text_primary)
            .child(self.render_sidebar(cx))
            .child(self.render_terminal_pane(cx));

        if self.palette_open {
            root.child(deferred(self.render_palette_overlay(cx)).with_priority(1))
        } else {
            root
        }
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
