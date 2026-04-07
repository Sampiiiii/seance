use std::sync::{Arc, Mutex, mpsc::Receiver};

use thiserror::Error;
use tokio::sync::oneshot;

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

pub struct SshConnectTask {
    pub session_id: u64,
    pub result_rx: Receiver<std::result::Result<SshConnectResult, SshError>>,
    pub abort_handle: SshConnectAbortHandle,
}

#[derive(Clone, Default)]
pub struct SshConnectAbortHandle {
    cancel_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl std::fmt::Debug for SshConnectAbortHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshConnectAbortHandle")
            .finish_non_exhaustive()
    }
}

impl SshConnectAbortHandle {
    pub(crate) fn new(cancel_tx: oneshot::Sender<()>) -> Self {
        Self {
            cancel_tx: Arc::new(Mutex::new(Some(cancel_tx))),
        }
    }

    pub fn abort(&self) -> bool {
        let Some(cancel_tx) = self
            .cancel_tx
            .lock()
            .expect("SSH connect abort handle poisoned")
            .take()
        else {
            return false;
        };

        cancel_tx.send(()).is_ok()
    }
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
    #[error("SSH connect timed out")]
    TimedOut,
    #[error("SSH connect cancelled")]
    Cancelled,
    #[error("SFTP bootstrap failed: {0}")]
    SftpBootstrap(String),
    #[error("no SFTP session for this connection")]
    SftpNotConnected,
    #[error("SFTP operation failed: {0}")]
    SftpOperation(String),
}
