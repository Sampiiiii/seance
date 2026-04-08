//! Root SSH API map: shared models, auth, session lifecycle, backend I/O, and SFTP helpers
//! live in dedicated modules while downstream crates keep importing from the crate root.

mod auth;
mod backend;
mod manager;
mod model;
mod proxy;
mod session;
mod sftp;
mod tunnel;

pub use manager::SshSessionManager;
pub use model::{
    PortForwardRuntimeSnapshot, PortForwardStatus, ResolvedAuthMethod, SftpBootstrapHandle,
    SftpEntry, SshConnectAbortHandle, SshConnectRequest, SshConnectResult, SshConnectTask,
    SshConnectionConfig, SshError, SshPortForwardHandle, SshPortForwardMode, SshPortForwardRequest,
};
pub use session::SshSessionHandle;
