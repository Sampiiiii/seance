// Owns terminal-specific key routing, IME composition state, and GPUI input-handler integration.

use std::ops::Range;

use gpui::{
    ClipboardItem, Context, EntityInputHandler, KeyDownEvent, Pixels, Point, UTF16Selection, Window,
};
use seance_terminal::{
    TerminalInputModifiers, TerminalKeyEvent, TerminalPaste, TerminalScreenKind, TerminalTextEvent,
};

use crate::{SeanceWorkspace, model::TerminalImeState, perf::RedrawReason};

pub(crate) fn terminal_text_event(event: &KeyDownEvent) -> Option<TerminalTextEvent> {
    let modifiers = event.keystroke.modifiers;
    let text = event.keystroke.key_char.as_ref()?;
    if text.is_empty()
        || modifiers.platform
        || modifiers.function
        || text.chars().any(char::is_control)
    {
        return None;
    }

    if modifiers.control && !modifiers.alt {
        return None;
    }

    Some(TerminalTextEvent {
        text: text.clone(),
        alt: modifiers.alt && !modifiers.control,
    })
}

pub(crate) fn terminal_key_event(event: &KeyDownEvent) -> Option<TerminalKeyEvent> {
    let key = event.keystroke.key.as_str();
    let modifiers = event.keystroke.modifiers;

    if modifiers.platform {
        return None;
    }

    if matches!(key, "shift" | "control" | "alt" | "platform" | "function") {
        return None;
    }

    if terminal_text_event(event).is_some() {
        return None;
    }

    Some(TerminalKeyEvent {
        key: key.to_string(),
        modifiers: TerminalInputModifiers {
            control: modifiers.control,
            alt: modifiers.alt,
            shift: modifiers.shift,
            platform: modifiers.platform,
            function: modifiers.function,
        },
    })
}

pub(crate) fn is_terminal_copy_shortcut(event: &KeyDownEvent) -> bool {
    let key = event.keystroke.key.as_str();
    let modifiers = event.keystroke.modifiers;
    #[cfg(target_os = "macos")]
    {
        modifiers.platform && !modifiers.control && !modifiers.alt && key == "c"
    }
    #[cfg(not(target_os = "macos"))]
    {
        modifiers.control && modifiers.shift && !modifiers.alt && key == "c"
    }
}

pub(crate) fn is_terminal_paste_shortcut(event: &KeyDownEvent) -> bool {
    let key = event.keystroke.key.as_str();
    let modifiers = event.keystroke.modifiers;
    #[cfg(target_os = "macos")]
    {
        modifiers.platform && !modifiers.control && !modifiers.alt && key == "v"
    }
    #[cfg(not(target_os = "macos"))]
    {
        modifiers.control && modifiers.shift && !modifiers.alt && key == "v"
    }
}

impl SeanceWorkspace {
    pub(crate) fn clear_terminal_ime(&mut self) {
        self.terminal_ime = TerminalImeState::default();
    }

    pub(crate) fn terminal_ime_visible(&self) -> bool {
        !self.terminal_ime.marked_text.is_empty()
    }

    pub(crate) fn handle_terminal_input_key(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(session) = self.active_session() else {
            return false;
        };
        let summary = session.summary();

        if is_terminal_copy_shortcut(event) {
            if let Some(selection) = self.terminal_selected_text() {
                cx.write_to_clipboard(ClipboardItem::new_string(selection));
                self.perf_overlay.mark_input(RedrawReason::Input);
                cx.notify();
            }
            cx.stop_propagation();
            return true;
        }

        if is_terminal_paste_shortcut(event) {
            if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                let _ = session.paste(TerminalPaste { text });
                self.perf_overlay.mark_input(RedrawReason::Input);
                cx.notify();
            }
            cx.stop_propagation();
            return true;
        }

        if let Some(text_event) = terminal_text_event(event)
            && (text_event.alt
                || (event.keystroke.modifiers.alt && event.keystroke.modifiers.control))
        {
            self.send_terminal_text_event(text_event, summary);
            cx.stop_propagation();
            self.perf_overlay.mark_input(RedrawReason::Input);
            cx.notify();
            return true;
        }

        if let Some(key_event) = terminal_key_event(event) {
            if matches!(summary.active_screen, TerminalScreenKind::Primary) && !summary.at_bottom {
                let _ = session.scroll_to_bottom();
            }
            let _ = session.send_key(key_event);
            cx.stop_propagation();
            self.perf_overlay.mark_input(RedrawReason::Input);
            cx.notify();
            return true;
        }

        false
    }

    pub(crate) fn send_terminal_text_event(
        &mut self,
        event: TerminalTextEvent,
        summary: seance_terminal::SessionSummary,
    ) {
        let Some(session) = self.active_session() else {
            return;
        };
        if matches!(summary.active_screen, TerminalScreenKind::Primary) && !summary.at_bottom {
            let _ = session.scroll_to_bottom();
        }
        let _ = session.send_text(event);
    }

    pub(crate) fn terminal_ime_overlay_position(&self) -> Option<(f32, f32)> {
        let cursor = self.terminal_surface.cursor?;
        let metrics = self.terminal_metrics?;
        let cursor_col = if cursor.position.at_wide_tail {
            cursor.position.x.saturating_sub(1)
        } else {
            cursor.position.x
        };
        Some((
            f32::from(cursor_col) * metrics.cell_width_px,
            f32::from(cursor.position.y) * metrics.line_height_px,
        ))
    }
}

impl EntityInputHandler for SeanceWorkspace {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        adjusted_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let len = utf16_len(&self.terminal_ime.marked_text);
        let start = range_utf16.start.min(len);
        let end = range_utf16.end.min(len);
        let range = start.min(end)..end.max(start);
        adjusted_range.replace(range.clone());
        Some(slice_utf16(&self.terminal_ime.marked_text, range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let caret = utf16_len(&self.terminal_ime.marked_text);
        Some(UTF16Selection {
            range: self
                .terminal_ime
                .marked_selected_range_utf16
                .clone()
                .unwrap_or(caret..caret),
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        (!self.terminal_ime.marked_text.is_empty())
            .then(|| 0..utf16_len(&self.terminal_ime.marked_text))
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.clear_terminal_ime();
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let committed = apply_ime_replacement(&self.terminal_ime.marked_text, range_utf16, text);
        let summary = self
            .active_session()
            .map(|session| session.summary())
            .unwrap_or_default();
        self.clear_terminal_ime();
        if !committed.is_empty() {
            self.send_terminal_text_event(
                TerminalTextEvent {
                    text: committed,
                    alt: false,
                },
                summary,
            );
        }
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.terminal_ime.marked_text =
            apply_ime_replacement(&self.terminal_ime.marked_text, range_utf16, new_text);
        self.terminal_ime.marked_selected_range_utf16 = new_selected_range_utf16
            .map(|range| clamp_range_utf16(&self.terminal_ime.marked_text, range));
        self.perf_overlay.mark_input(RedrawReason::Input);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        element_bounds: gpui::Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::Bounds<Pixels>> {
        let metrics = self.terminal_metrics?;
        let (origin_x, origin_y) = self.terminal_ime_overlay_position().unwrap_or((0.0, 0.0));
        let prefix_cols = utf16_len(&slice_utf16(
            &self.terminal_ime.marked_text,
            0..range_utf16
                .start
                .min(utf16_len(&self.terminal_ime.marked_text)),
        )) as f32;
        let width_cols = utf16_len(&slice_utf16(
            &self.terminal_ime.marked_text,
            range_utf16
                .start
                .min(utf16_len(&self.terminal_ime.marked_text))
                ..range_utf16
                    .end
                    .min(utf16_len(&self.terminal_ime.marked_text)),
        ))
        .max(1) as f32;
        Some(gpui::Bounds::new(
            gpui::point(
                element_bounds.origin.x + gpui::px(origin_x + prefix_cols * metrics.cell_width_px),
                element_bounds.origin.y + gpui::px(origin_y),
            ),
            gpui::size(
                gpui::px(width_cols * metrics.cell_width_px),
                gpui::px(metrics.line_height_px),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        Some(0)
    }
}

fn clamp_range_utf16(text: &str, range: Range<usize>) -> Range<usize> {
    let len = utf16_len(text);
    let start = range.start.min(len);
    let end = range.end.min(len);
    start.min(end)..end.max(start)
}

fn apply_ime_replacement(current: &str, range_utf16: Option<Range<usize>>, text: &str) -> String {
    let range = clamp_range_utf16(current, range_utf16.unwrap_or(0..utf16_len(current)));
    let start = utf16_to_byte(current, range.start);
    let end = utf16_to_byte(current, range.end);
    let mut replaced = current.to_string();
    replaced.replace_range(start..end, text);
    replaced
}

fn slice_utf16(text: &str, range_utf16: Range<usize>) -> String {
    let range = clamp_range_utf16(text, range_utf16);
    let start = utf16_to_byte(text, range.start);
    let end = utf16_to_byte(text, range.end);
    text[start..end].to_string()
}

fn utf16_to_byte(text: &str, utf16_index: usize) -> usize {
    let mut consumed = 0;
    for (byte_idx, ch) in text.char_indices() {
        if consumed >= utf16_index {
            return byte_idx;
        }
        consumed += ch.len_utf16();
    }
    text.len()
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Keystroke, Modifiers};

    fn key_event(key: &str, key_char: Option<&str>, modifiers: Modifiers) -> KeyDownEvent {
        KeyDownEvent {
            keystroke: Keystroke {
                modifiers,
                key: key.into(),
                key_char: key_char.map(ToOwned::to_owned),
            },
            is_held: false,
        }
    }

    #[test]
    fn terminal_text_event_extracts_printable_text() {
        let event = key_event("@", Some("@"), Modifiers::shift());

        assert_eq!(
            terminal_text_event(&event),
            Some(TerminalTextEvent {
                text: "@".into(),
                alt: false,
            })
        );
    }

    #[test]
    fn terminal_text_event_keeps_alt_for_terminal_text() {
        let event = key_event("2", Some("@"), Modifiers::alt() | Modifiers::shift());

        assert_eq!(
            terminal_text_event(&event),
            Some(TerminalTextEvent {
                text: "@".into(),
                alt: true,
            })
        );
    }

    #[test]
    fn terminal_key_event_keeps_control_combos_as_keys() {
        let event = key_event("c", Some("c"), Modifiers::control());

        assert_eq!(
            terminal_key_event(&event),
            Some(TerminalKeyEvent {
                key: "c".into(),
                modifiers: TerminalInputModifiers {
                    control: true,
                    ..TerminalInputModifiers::default()
                },
            })
        );
    }

    #[test]
    fn ime_replacement_updates_marked_text_ranges() {
        assert_eq!(apply_ime_replacement("caf", Some(1..2), "a"), "caf");
        assert_eq!(apply_ime_replacement("caf", Some(1..3), "fé"), "cfé");
    }
}
