//! Root terminal API map: terminal models, session state, rendering, and local PTY handling
//! live in dedicated modules while downstream crates keep importing from the crate root.

mod history;
mod input;
mod local;
mod model;
mod render;
mod state;
mod viewport;

pub use history::{
    DroppedEventCounter, NoopTranscriptSink, TerminalTranscriptSink, TranscriptEvent,
    TranscriptStream,
};
pub use input::{
    TerminalInputModifiers, TerminalKeyEvent, TerminalMouseButton, TerminalMouseEvent,
    TerminalMouseEventKind, TerminalPaste, TerminalTextEvent,
};
pub use local::{LocalSessionFactory, LocalSessionHandle};
pub use model::{
    SessionSummary, TerminalCell, TerminalCellStyle, TerminalColor, TerminalCursor,
    TerminalCursorState, TerminalCursorVisualStyle, TerminalGeometry, TerminalPixelSize,
    TerminalRow, TerminalScreenKind, TerminalScrollCommand, TerminalScrollbarState, TerminalSize,
    TerminalViewportSnapshot,
};
pub use render::TerminalEmulator;
pub use state::{
    GhosttyDirtyState, SessionPerfSnapshot, SharedSessionState, TerminalRenderMetrics,
    TerminalSession, next_session_id,
};
