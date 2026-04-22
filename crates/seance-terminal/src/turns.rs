// Owns in-memory terminal turn tracking based on submitted input lines and captured output.

use std::collections::VecDeque;

use tracing::trace;

use crate::TerminalTurnSnapshot;

const DEFAULT_MAX_TURNS: usize = 256;
const DEFAULT_MAX_BYTES_PER_TURN: usize = 256 * 1024;
const TRUNCATION_MARKER: &str = "\n...[truncated]";

const ESCAPE: u8 = 0x1b;
const BACKSPACE: u8 = 0x08;
const DELETE: u8 = 0x7f;
const CTRL_U: u8 = 0x15;

#[derive(Clone, Debug, PartialEq, Eq)]
struct TurnRecord {
    turn_id: u64,
    command_text: String,
    output_text: String,
    start_row: u64,
    end_row: u64,
    command_truncated: bool,
    output_truncated: bool,
}

impl TurnRecord {
    fn snapshot(&self) -> TerminalTurnSnapshot {
        let combined_text = if self.output_text.is_empty() {
            self.command_text.clone()
        } else {
            format!("{}\n{}", self.command_text, self.output_text)
        };
        TerminalTurnSnapshot {
            turn_id: self.turn_id,
            command_text: self.command_text.clone(),
            output_text: self.output_text.clone(),
            combined_text,
            start_row: self.start_row,
            end_row: self.end_row,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum EscapeState {
    #[default]
    Ground,
    Escape,
    Csi,
    Ss3,
}

#[derive(Debug)]
pub(crate) struct TurnTracker {
    completed_turns: VecDeque<TurnRecord>,
    active_turn: Option<TurnRecord>,
    input_line_buffer: String,
    input_escape_state: EscapeState,
    output_escape_state: EscapeState,
    latest_cursor_abs_row: u64,
    capture_primary_screen: bool,
    next_turn_id: u64,
    max_turns: usize,
    max_bytes_per_turn: usize,
}

impl Default for TurnTracker {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_TURNS, DEFAULT_MAX_BYTES_PER_TURN)
    }
}

impl TurnTracker {
    pub(crate) fn new(max_turns: usize, max_bytes_per_turn: usize) -> Self {
        Self {
            completed_turns: VecDeque::new(),
            active_turn: None,
            input_line_buffer: String::new(),
            input_escape_state: EscapeState::default(),
            output_escape_state: EscapeState::default(),
            latest_cursor_abs_row: 0,
            capture_primary_screen: true,
            next_turn_id: 1,
            max_turns: max_turns.max(1),
            max_bytes_per_turn: max_bytes_per_turn.max(TRUNCATION_MARKER.len()),
        }
    }

    pub(crate) fn set_primary_screen(&mut self, primary: bool) {
        self.capture_primary_screen = primary;
    }

    pub(crate) fn update_cursor_abs_row(&mut self, cursor_abs_row: u64) {
        self.latest_cursor_abs_row = cursor_abs_row;
        if let Some(turn) = self.active_turn.as_mut() {
            turn.end_row = cursor_abs_row;
        }
    }

    pub(crate) fn track_input_bytes(&mut self, bytes: &[u8]) {
        if !self.capture_primary_screen {
            return;
        }

        for &byte in bytes {
            match self.input_escape_state {
                EscapeState::Ground => self.process_input_ground_byte(byte),
                EscapeState::Escape => self.process_input_escape_byte(byte),
                EscapeState::Csi => {
                    if csi_sequence_complete(byte) {
                        self.input_escape_state = EscapeState::Ground;
                    }
                }
                EscapeState::Ss3 => {
                    self.input_escape_state = EscapeState::Ground;
                }
            }
        }
    }

    pub(crate) fn track_output_bytes(&mut self, bytes: &[u8]) {
        if !self.capture_primary_screen {
            return;
        }

        let Some(active_turn) = self.active_turn.as_mut() else {
            return;
        };

        let mut sanitized = String::new();
        for &byte in bytes {
            match self.output_escape_state {
                EscapeState::Ground => {
                    if byte == ESCAPE {
                        self.output_escape_state = EscapeState::Escape;
                        continue;
                    }
                    match byte {
                        b'\r' => {}
                        b'\n' => sanitized.push('\n'),
                        BACKSPACE | DELETE => {
                            sanitized.pop();
                        }
                        0x20..=0x7e => sanitized.push(byte as char),
                        0x80..=0xff => append_lossy_byte(&mut sanitized, byte),
                        _ => {}
                    }
                }
                EscapeState::Escape => match byte {
                    b'[' => self.output_escape_state = EscapeState::Csi,
                    b'O' => self.output_escape_state = EscapeState::Ss3,
                    _ => self.output_escape_state = EscapeState::Ground,
                },
                EscapeState::Csi => {
                    if csi_sequence_complete(byte) {
                        self.output_escape_state = EscapeState::Ground;
                    }
                }
                EscapeState::Ss3 => {
                    self.output_escape_state = EscapeState::Ground;
                }
            }
        }

        if sanitized.is_empty() {
            return;
        }

        append_bounded_text(
            &mut active_turn.output_text,
            &sanitized,
            self.max_bytes_per_turn,
            &mut active_turn.output_truncated,
        );
        if active_turn.output_truncated {
            trace!(
                turn_id = active_turn.turn_id,
                max_bytes = self.max_bytes_per_turn,
                "terminal turn output truncated"
            );
        }
    }

    pub(crate) fn previous_turn_snapshot(&self) -> Option<TerminalTurnSnapshot> {
        self.active_turn
            .as_ref()
            .or_else(|| self.completed_turns.back())
            .map(TurnRecord::snapshot)
    }

    fn process_input_ground_byte(&mut self, byte: u8) {
        match byte {
            ESCAPE => {
                self.input_escape_state = EscapeState::Escape;
            }
            b'\r' | b'\n' => {
                self.submit_command();
            }
            BACKSPACE | DELETE => {
                self.input_line_buffer.pop();
            }
            CTRL_U => {
                self.input_line_buffer.clear();
            }
            0x20..=0x7e => self.input_line_buffer.push(byte as char),
            0x80..=0xff => append_lossy_byte(&mut self.input_line_buffer, byte),
            _ => {}
        }
    }

    fn process_input_escape_byte(&mut self, byte: u8) {
        match byte {
            b'[' => self.input_escape_state = EscapeState::Csi,
            b'O' => self.input_escape_state = EscapeState::Ss3,
            _ => self.input_escape_state = EscapeState::Ground,
        }
    }

    fn submit_command(&mut self) {
        let command = self.input_line_buffer.trim_end().to_string();
        self.input_line_buffer.clear();
        self.input_escape_state = EscapeState::Ground;

        if command.is_empty() {
            return;
        }

        if let Some(previous_active) = self.active_turn.take() {
            trace!(
                turn_id = previous_active.turn_id,
                start_row = previous_active.start_row,
                end_row = previous_active.end_row,
                "terminal turn finalized"
            );
            self.completed_turns.push_back(previous_active);
            self.enforce_history_limit();
        }

        let turn_id = self.next_turn_id;
        self.next_turn_id = self.next_turn_id.saturating_add(1);
        let mut turn = TurnRecord {
            turn_id,
            command_text: String::new(),
            output_text: String::new(),
            start_row: self.latest_cursor_abs_row,
            end_row: self.latest_cursor_abs_row,
            command_truncated: false,
            output_truncated: false,
        };
        append_bounded_text(
            &mut turn.command_text,
            &command,
            self.max_bytes_per_turn,
            &mut turn.command_truncated,
        );
        if turn.command_truncated {
            trace!(
                turn_id,
                max_bytes = self.max_bytes_per_turn,
                "terminal turn command truncated"
            );
        }
        trace!(
            turn_id,
            start_row = turn.start_row,
            "terminal turn submitted"
        );
        self.active_turn = Some(turn);
    }

    fn enforce_history_limit(&mut self) {
        while self.completed_turns.len() > self.max_turns {
            if let Some(dropped) = self.completed_turns.pop_front() {
                trace!(
                    dropped_turn_id = dropped.turn_id,
                    max_turns = self.max_turns,
                    "terminal turn history truncated"
                );
            }
        }
    }
}

fn append_lossy_byte(target: &mut String, byte: u8) {
    target.push_str(String::from_utf8_lossy(&[byte]).as_ref());
}

fn append_bounded_text(
    buffer: &mut String,
    incoming: &str,
    max_bytes: usize,
    already_truncated: &mut bool,
) {
    if incoming.is_empty() || *already_truncated {
        return;
    }

    if buffer.len() >= max_bytes {
        *already_truncated = true;
        append_truncation_marker(buffer, max_bytes);
        return;
    }

    let remaining = max_bytes - buffer.len();
    if incoming.len() <= remaining {
        buffer.push_str(incoming);
        return;
    }

    let marker_reserve = TRUNCATION_MARKER.len().min(remaining);
    let cutoff_budget = remaining.saturating_sub(marker_reserve);
    let cutoff = floor_char_boundary(incoming, cutoff_budget);
    if cutoff > 0 {
        buffer.push_str(&incoming[..cutoff]);
    }
    *already_truncated = true;
    append_truncation_marker(buffer, max_bytes);
}

fn append_truncation_marker(buffer: &mut String, max_bytes: usize) {
    if buffer.len() >= max_bytes {
        return;
    }

    let remaining = max_bytes - buffer.len();
    if TRUNCATION_MARKER.len() <= remaining {
        buffer.push_str(TRUNCATION_MARKER);
        return;
    }

    let cutoff = floor_char_boundary(TRUNCATION_MARKER, remaining);
    if cutoff > 0 {
        buffer.push_str(&TRUNCATION_MARKER[..cutoff]);
    }
}

fn floor_char_boundary(text: &str, max_len: usize) -> usize {
    if max_len >= text.len() {
        return text.len();
    }

    let mut boundary = 0;
    for (idx, ch) in text.char_indices() {
        let next = idx + ch.len_utf8();
        if next > max_len {
            break;
        }
        boundary = next;
    }
    boundary
}

fn csi_sequence_complete(byte: u8) -> bool {
    (0x40..=0x7e).contains(&byte)
}

#[cfg(test)]
mod tests {
    use super::TurnTracker;

    #[test]
    fn tracks_submitted_command_and_output_bytes() {
        let mut tracker = TurnTracker::new(32, 1_024);
        tracker.update_cursor_abs_row(12);
        tracker.track_input_bytes(b"echo hi\r");
        tracker.track_output_bytes(b"hello\nworld\n");
        tracker.update_cursor_abs_row(14);

        let turn = tracker.previous_turn_snapshot().expect("turn");
        assert_eq!(turn.command_text, "echo hi");
        assert_eq!(turn.output_text, "hello\nworld\n");
        assert_eq!(turn.start_row, 12);
        assert_eq!(turn.end_row, 14);
        assert!(turn.combined_text.starts_with("echo hi\nhello"));
    }

    #[test]
    fn applies_backspace_and_ctrl_u_while_tracking_input_line() {
        let mut tracker = TurnTracker::new(32, 1_024);

        tracker.track_input_bytes(b"alphx");
        tracker.track_input_bytes(&[0x7f]);
        tracker.track_input_bytes(b"a\r");

        let first = tracker.previous_turn_snapshot().expect("first turn");
        assert_eq!(first.command_text, "alpha");

        tracker.track_input_bytes(b"beta");
        tracker.track_input_bytes(&[0x15]);
        tracker.track_input_bytes(b"gamma\r");
        let second = tracker.previous_turn_snapshot().expect("second turn");
        assert_eq!(second.command_text, "gamma");
    }

    #[test]
    fn ignores_capture_while_not_on_primary_screen() {
        let mut tracker = TurnTracker::new(32, 1_024);
        tracker.set_primary_screen(false);
        tracker.track_input_bytes(b"ignored\r");
        tracker.track_output_bytes(b"noise");

        assert!(tracker.previous_turn_snapshot().is_none());
    }

    #[test]
    fn truncates_large_turn_content_with_marker() {
        let mut tracker = TurnTracker::new(32, 16);
        tracker.track_input_bytes(b"very-long-command-text\r");
        tracker.track_output_bytes(b"very-long-output-text");

        let turn = tracker.previous_turn_snapshot().expect("turn");
        assert!(turn.command_text.contains("truncated"));
        assert!(turn.output_text.contains("truncated"));
    }
}
