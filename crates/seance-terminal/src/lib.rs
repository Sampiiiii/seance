//! Root terminal API map: terminal models, session state, rendering, and local PTY handling
//! live in dedicated modules while downstream crates keep importing from the crate root.

mod local;
mod model;
mod render;
mod state;

pub use local::{LocalSessionFactory, LocalSessionHandle};
pub use model::{
    TerminalCell, TerminalCellStyle, TerminalColor, TerminalGeometry, TerminalPixelSize,
    TerminalRow, TerminalSize,
};
pub use render::TerminalEmulator;
pub use state::{
    SessionPerfSnapshot, SessionSnapshot, SharedSessionState, TerminalRenderMetrics,
    TerminalSession, next_session_id,
};
