//! Root SSH API map: shared models, auth, session lifecycle, backend I/O, and SFTP helpers
//! live in dedicated modules while downstream crates keep importing from the crate root.

mod auth;
mod backend;
mod manager;
mod model;
mod session;
mod sftp;

pub use manager::SshSessionManager;
pub use model::{
    ResolvedAuthMethod, SftpBootstrapHandle, SftpEntry, SshConnectRequest, SshConnectResult,
    SshConnectionConfig, SshError,
};
pub use session::SshSessionHandle;
