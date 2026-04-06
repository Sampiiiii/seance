use std::sync::Arc;

use thiserror::Error;

use crate::session::SshSessionHandle;

#[derive(Debug, Clone)]
pub struct SshConnectionConfig {
    pub label: String,
    pub hostname: String,
    pub port: u16,
    pub username: String,
}

#[derive(Debug, Clone)]
pub enum ResolvedAuthMethod {
    Password {
        password: String,
    },
    PrivateKey {
        private_key_pem: String,
        passphrase: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct SshConnectRequest {
    pub connection: SshConnectionConfig,
    pub auth_order: Vec<ResolvedAuthMethod>,
}

#[derive(Debug, Clone)]
pub struct SftpBootstrapHandle {
    pub session_id: u64,
    pub ready: bool,
}

#[derive(Debug, Clone)]
pub struct SftpEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<u32>,
    pub permissions: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct SshConnectResult {
    pub session: Arc<SshSessionHandle>,
    pub sftp: SftpBootstrapHandle,
}

#[derive(Debug, Error)]
pub enum SshError {
    #[error("no SSH auth methods were resolved for this host")]
    MissingAuthMethods,
    #[error("all configured SSH auth methods were rejected by the server")]
    AuthenticationRejected,
    #[error("invalid private key configuration: {0}")]
    InvalidPrivateKey(String),
    #[error("SSH transport error: {0}")]
    Transport(String),
    #[error("SFTP bootstrap failed: {0}")]
    SftpBootstrap(String),
    #[error("no SFTP session for this connection")]
    SftpNotConnected,
    #[error("SFTP operation failed: {0}")]
    SftpOperation(String),
}
