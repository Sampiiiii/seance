use anyhow::{Result, anyhow};
use libghostty_vt::{key, mouse, terminal::Mode};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TerminalInputModifiers {
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
    pub platform: bool,
    pub function: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalKeyEvent {
    pub key: String,
    pub modifiers: TerminalInputModifiers,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalTextEvent {
    pub text: String,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalMouseButton {
    Left,
    Right,
    Middle,
    WheelUp,
    WheelDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalMouseEventKind {
    Press,
    Release,
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalMouseEvent {
    pub kind: TerminalMouseEventKind,
    pub button: Option<TerminalMouseButton>,
    pub x_px: u32,
    pub y_px: u32,
    pub modifiers: TerminalInputModifiers,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalPaste {
    pub text: String,
}

pub(crate) fn modifiers_to_ghostty(modifiers: TerminalInputModifiers) -> key::Mods {
    let mut mods = key::Mods::empty();
    if modifiers.shift {
        mods |= key::Mods::SHIFT;
    }
    if modifiers.alt {
        mods |= key::Mods::ALT;
    }
    if modifiers.control {
        mods |= key::Mods::CTRL;
    }
    if modifiers.platform {
        mods |= key::Mods::SUPER;
    }
    mods
}

pub(crate) fn map_terminal_key(key: &str) -> Result<key::Key> {
    let key = key.to_ascii_lowercase();
    let mapped = match key.as_str() {
        "a" => key::Key::A,
        "b" => key::Key::B,
        "c" => key::Key::C,
        "d" => key::Key::D,
        "e" => key::Key::E,
        "f" => key::Key::F,
        "g" => key::Key::G,
        "h" => key::Key::H,
        "i" => key::Key::I,
        "j" => key::Key::J,
        "k" => key::Key::K,
        "l" => key::Key::L,
        "m" => key::Key::M,
        "n" => key::Key::N,
        "o" => key::Key::O,
        "p" => key::Key::P,
        "q" => key::Key::Q,
        "r" => key::Key::R,
        "s" => key::Key::S,
        "t" => key::Key::T,
        "u" => key::Key::U,
        "v" => key::Key::V,
        "w" => key::Key::W,
        "x" => key::Key::X,
        "y" => key::Key::Y,
        "z" => key::Key::Z,
        "0" => key::Key::Digit0,
        "1" => key::Key::Digit1,
        "2" => key::Key::Digit2,
        "3" => key::Key::Digit3,
        "4" => key::Key::Digit4,
        "5" => key::Key::Digit5,
        "6" => key::Key::Digit6,
        "7" => key::Key::Digit7,
        "8" => key::Key::Digit8,
        "9" => key::Key::Digit9,
        "`" => key::Key::Backquote,
        "\\" => key::Key::Backslash,
        "[" => key::Key::BracketLeft,
        "]" => key::Key::BracketRight,
        "," => key::Key::Comma,
        "=" => key::Key::Equal,
        "-" => key::Key::Minus,
        "." => key::Key::Period,
        "'" => key::Key::Quote,
        ";" => key::Key::Semicolon,
        "/" => key::Key::Slash,
        "backspace" => key::Key::Backspace,
        "delete" => key::Key::Delete,
        "down" => key::Key::ArrowDown,
        "end" => key::Key::End,
        "enter" => key::Key::Enter,
        "escape" => key::Key::Escape,
        "f1" => key::Key::F1,
        "f2" => key::Key::F2,
        "f3" => key::Key::F3,
        "f4" => key::Key::F4,
        "f5" => key::Key::F5,
        "f6" => key::Key::F6,
        "f7" => key::Key::F7,
        "f8" => key::Key::F8,
        "f9" => key::Key::F9,
        "f10" => key::Key::F10,
        "f11" => key::Key::F11,
        "f12" => key::Key::F12,
        "home" => key::Key::Home,
        "insert" => key::Key::Insert,
        "left" => key::Key::ArrowLeft,
        "pagedown" => key::Key::PageDown,
        "pageup" => key::Key::PageUp,
        "right" => key::Key::ArrowRight,
        "space" => key::Key::Space,
        "tab" => key::Key::Tab,
        "up" => key::Key::ArrowUp,
        _ => return Err(anyhow!("unsupported terminal key: {key}")),
    };
    Ok(mapped)
}

pub(crate) fn encode_text_event(event: &TerminalTextEvent) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(event.text.len() + usize::from(event.alt));
    if event.alt {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(event.text.as_bytes());
    bytes
}

pub(crate) fn map_mouse_button(button: TerminalMouseButton) -> mouse::Button {
    match button {
        TerminalMouseButton::Left => mouse::Button::Left,
        TerminalMouseButton::Right => mouse::Button::Right,
        TerminalMouseButton::Middle => mouse::Button::Middle,
        TerminalMouseButton::WheelUp => mouse::Button::Four,
        TerminalMouseButton::WheelDown => mouse::Button::Five,
    }
}

pub(crate) fn mouse_kind(kind: TerminalMouseEventKind) -> mouse::Action {
    match kind {
        TerminalMouseEventKind::Press => mouse::Action::Press,
        TerminalMouseEventKind::Release => mouse::Action::Release,
        TerminalMouseEventKind::Move => mouse::Action::Motion,
    }
}

pub(crate) fn encode_bracketed_paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        let mut bytes = Vec::with_capacity(text.len() + 12);
        bytes.extend_from_slice(b"\x1b[200~");
        bytes.extend_from_slice(text.as_bytes());
        bytes.extend_from_slice(b"\x1b[201~");
        bytes
    } else {
        text.as_bytes().to_vec()
    }
}

pub(crate) fn bracketed_paste_enabled(terminal: &libghostty_vt::Terminal<'_, '_>) -> bool {
    terminal.mode(Mode::BRACKETED_PASTE).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_text_event_supports_printable_symbols() {
        let bytes = encode_text_event(&TerminalTextEvent {
            text: "@".into(),
            alt: false,
        });

        assert_eq!(bytes, b"@");
    }

    #[test]
    fn encode_text_event_prefixes_alt_sequences() {
        let bytes = encode_text_event(&TerminalTextEvent {
            text: "@".into(),
            alt: true,
        });

        assert_eq!(bytes, b"\x1b@");
    }

    #[test]
    fn encode_text_event_supports_utf8_text() {
        let bytes = encode_text_event(&TerminalTextEvent {
            text: "é🙂".into(),
            alt: false,
        });

        assert_eq!(bytes, "é🙂".as_bytes());
    }
}
