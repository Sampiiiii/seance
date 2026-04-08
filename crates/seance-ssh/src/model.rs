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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SshPortForwardMode {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortForwardStatus {
    Starting,
    Running,
    Failed,
}

#[derive(Debug, Clone)]
pub struct SshPortForwardRequest {
    pub id: String,
    pub vault_id: String,
    pub forward_id: String,
    pub host_id: String,
    pub label: String,
    pub host_label: String,
    pub mode: SshPortForwardMode,
    pub listen_address: String,
    pub listen_port: u16,
    pub target_address: String,
    pub target_port: u16,
    pub connection: SshConnectionConfig,
    pub auth_order: Vec<ResolvedAuthMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortForwardRuntimeSnapshot {
    pub id: String,
    pub vault_id: String,
    pub forward_id: String,
    pub host_id: String,
    pub label: String,
    pub host_label: String,
    pub mode: SshPortForwardMode,
    pub status: PortForwardStatus,
    pub listen_address: String,
    pub listen_port: u16,
    pub target_address: String,
    pub target_port: u16,
    pub opened_at: Option<i64>,
    pub active_connections: usize,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub last_error: Option<String>,
}

#[derive(Clone, Default)]
pub struct SshPortForwardHandle {
    cancel_tx: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

impl std::fmt::Debug for SshPortForwardHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshPortForwardHandle")
            .finish_non_exhaustive()
    }
}

impl SshPortForwardHandle {
    pub(crate) fn new(cancel_tx: oneshot::Sender<()>) -> Self {
        Self {
            cancel_tx: Arc::new(Mutex::new(Some(cancel_tx))),
        }
    }

    pub fn abort(&self) -> bool {
        let Some(cancel_tx) = self
            .cancel_tx
            .lock()
            .expect("SSH port forward handle poisoned")
            .take()
        else {
            return false;
        };

        cancel_tx.send(()).is_ok()
    }
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
    #[error("port forward {0} is already running")]
    PortForwardAlreadyRunning(String),
    #[error("port forward listener failed: {0}")]
    PortForwardBind(String),
    #[error("port forward target connect failed: {0}")]
    PortForwardTargetConnect(String),
    #[error("port forward channel open failed: {0}")]
    PortForwardChannel(String),
}
