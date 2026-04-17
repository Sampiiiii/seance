// Owns shared text editing state for custom GPUI fields that don't use a native text input.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TextEditState {
    anchor: usize,
    focus: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TextDisplayFragments {
    pub(crate) prefix: String,
    pub(crate) selected: Option<String>,
    pub(crate) suffix: String,
    pub(crate) caret_visible: bool,
}

impl TextEditState {
    pub(crate) fn with_text(text: &str) -> Self {
        let len = text.chars().count();
        Self {
            anchor: len,
            focus: len,
        }
    }

    pub(crate) fn sync(&mut self, text: &str) {
        let len = text.chars().count();
        self.anchor = self.anchor.min(len);
        self.focus = self.focus.min(len);
    }

    pub(crate) fn move_left(&mut self, text: &str, extend: bool) {
        self.sync(text);
        if !extend && self.has_selection() {
            let (start, _) = self.selection_range();
            self.anchor = start;
            self.focus = start;
            return;
        }
        self.focus = self.focus.saturating_sub(1);
        if !extend {
            self.anchor = self.focus;
        }
    }

    pub(crate) fn move_right(&mut self, text: &str, extend: bool) {
        self.sync(text);
        if !extend && self.has_selection() {
            let (_, end) = self.selection_range();
            self.anchor = end;
            self.focus = end;
            return;
        }
        let len = text.chars().count();
        self.focus = (self.focus + 1).min(len);
        if !extend {
            self.anchor = self.focus;
        }
    }

    pub(crate) fn move_home(&mut self, extend: bool) {
        self.focus = 0;
        if !extend {
            self.anchor = 0;
        }
    }

    pub(crate) fn move_end(&mut self, text: &str, extend: bool) {
        let len = text.chars().count();
        self.focus = len;
        if !extend {
            self.anchor = len;
        }
    }

    pub(crate) fn select_all(&mut self, text: &str) {
        self.anchor = 0;
        self.focus = text.chars().count();
    }

    pub(crate) fn insert_text(&mut self, text: &mut String, input: &str) {
        self.replace_selection(text, input);
    }

    pub(crate) fn backspace(&mut self, text: &mut String) {
        self.sync(text);
        if self.has_selection() {
            self.replace_selection(text, "");
            return;
        }
        if self.focus == 0 {
            return;
        }
        let start = self.focus - 1;
        replace_char_range(text, start, self.focus, "");
        self.anchor = start;
        self.focus = start;
    }

    pub(crate) fn delete_forward(&mut self, text: &mut String) {
        self.sync(text);
        if self.has_selection() {
            self.replace_selection(text, "");
            return;
        }
        let len = text.chars().count();
        if self.focus >= len {
            return;
        }
        replace_char_range(text, self.focus, self.focus + 1, "");
        self.anchor = self.focus;
    }

    pub(crate) fn copy(&self, text: &str) -> Option<String> {
        self.has_selection()
            .then(|| {
                let (start, end) = self.selection_range();
                slice_chars(text, start, end)
            })
            .filter(|value| !value.is_empty())
    }

    pub(crate) fn cut(&mut self, text: &mut String) -> Option<String> {
        let copied = self.copy(text)?;
        self.replace_selection(text, "");
        Some(copied)
    }

    pub(crate) fn display_fragments(&self, text: &str) -> TextDisplayFragments {
        let len = text.chars().count();
        let (start, end) = self.clamped_range(len);
        if start != end {
            return TextDisplayFragments {
                prefix: slice_chars(text, 0, start),
                selected: Some(slice_chars(text, start, end)),
                suffix: slice_chars(text, end, len),
                caret_visible: false,
            };
        }

        TextDisplayFragments {
            prefix: slice_chars(text, 0, end),
            selected: None,
            suffix: slice_chars(text, end, len),
            caret_visible: true,
        }
    }

    pub(crate) fn has_selection(&self) -> bool {
        self.anchor != self.focus
    }

    pub(crate) fn selection_range(&self) -> (usize, usize) {
        if self.anchor <= self.focus {
            (self.anchor, self.focus)
        } else {
            (self.focus, self.anchor)
        }
    }

    fn clamped_range(&self, len: usize) -> (usize, usize) {
        let (start, end) = self.selection_range();
        (start.min(len), end.min(len))
    }

    fn replace_selection(&mut self, text: &mut String, replacement: &str) {
        self.sync(text);
        let (start, end) = self.selection_range();
        replace_char_range(text, start, end, replacement);
        let next = start + replacement.chars().count();
        self.anchor = next;
        self.focus = next;
    }
}

fn replace_char_range(text: &mut String, start: usize, end: usize, replacement: &str) {
    let start_byte = char_to_byte_index(text, start);
    let end_byte = char_to_byte_index(text, end);
    text.replace_range(start_byte..end_byte, replacement);
}

fn slice_chars(text: &str, start: usize, end: usize) -> String {
    let start_byte = char_to_byte_index(text, start);
    let end_byte = char_to_byte_index(text, end);
    text[start_byte..end_byte].to_string()
}

fn char_to_byte_index(text: &str, char_index: usize) -> usize {
    text.char_indices()
        .nth(char_index)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::TextEditState;

    #[test]
    fn insert_and_navigation_follow_selection() {
        let mut text = String::from("hello");
        let mut state = TextEditState::with_text(&text);
        state.move_left(&text, false);
        state.insert_text(&mut text, "!");
        assert_eq!(text, "hell!o");
    }

    #[test]
    fn select_all_cut_and_paste_work() {
        let mut text = String::from("hello");
        let mut state = TextEditState::with_text(&text);
        state.select_all(&text);
        assert_eq!(state.cut(&mut text).as_deref(), Some("hello"));
        assert!(text.is_empty());
        state.insert_text(&mut text, "world");
        assert_eq!(text, "world");
    }

    #[test]
    fn delete_forward_removes_next_character() {
        let mut text = String::from("hello");
        let mut state = TextEditState::with_text(&text);
        state.move_home(false);
        state.delete_forward(&mut text);
        assert_eq!(text, "ello");
    }
}
