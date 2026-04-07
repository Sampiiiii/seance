use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::{Context, Result};
use russh::client;
use russh_sftp::client::SftpSession;
use seance_terminal::{SharedSessionState, TerminalGeometry, next_session_id};
use tokio::{
    runtime::Runtime,
    sync::{mpsc, oneshot},
};
use tracing::{debug, trace, warn};

use crate::{
    auth::{AcceptAnyHostKeyHandler, authenticate},
    backend::run_ssh_session,
    model::{
        SftpBootstrapHandle, SshConnectAbortHandle, SshConnectRequest, SshConnectResult,
        SshConnectTask, SshError,
    },
    session::{SessionCommand, SshSessionHandle},
};

pub struct SshSessionManager {
    pub(crate) runtime: Runtime,
    pub(crate) sftp_sessions: Arc<Mutex<HashMap<u64, Arc<tokio::sync::Mutex<SftpSession>>>>>,
}

const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

const _: () = {
    fn _assert_send_sync<T: Send + Sync>() {}
    fn _assert() {
        _assert_send_sync::<SshSessionManager>();
    }
};

impl SshSessionManager {
    pub fn new() -> Result<Self> {
        Ok(Self {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("failed to initialize SSH runtime")?,
            sftp_sessions: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn connect(
        &self,
        request: SshConnectRequest,
    ) -> std::result::Result<SshConnectResult, SshError> {
        if request.auth_order.is_empty() {
            return Err(SshError::MissingAuthMethods);
        }

        let session_id = next_session_id();
        let sftp_sessions = Arc::clone(&self.sftp_sessions);
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        self.runtime.block_on(Self::run_connect_future(
            session_id,
            Self::connect_async(session_id, request, Arc::clone(&sftp_sessions)),
            cancel_rx,
            CONNECT_TIMEOUT,
            &sftp_sessions,
        ))
    }

    pub fn sftp_ready(&self, session_id: u64) -> bool {
        self.sftp_sessions
            .lock()
            .expect("sftp session map poisoned")
            .contains_key(&session_id)
    }

    pub fn start_connect(
        &self,
        request: SshConnectRequest,
    ) -> std::result::Result<SshConnectTask, SshError> {
        if request.auth_order.is_empty() {
            return Err(SshError::MissingAuthMethods);
        }

        let session_id = next_session_id();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let sftp_sessions = Arc::clone(&self.sftp_sessions);
        let host = format!(
            "{}@{}:{}",
            request.connection.username, request.connection.hostname, request.connection.port
        );

        trace!(session_id, host = %host, "starting SSH connect task");
        let _ = self.runtime.spawn(async move {
            let result = Self::run_connect_future(
                session_id,
                Self::connect_async(session_id, request, Arc::clone(&sftp_sessions)),
                cancel_rx,
                CONNECT_TIMEOUT,
                &sftp_sessions,
            )
            .await;
            let _ = result_tx.send(result);
        });

        Ok(SshConnectTask {
            session_id,
            result_rx,
            abort_handle: SshConnectAbortHandle::new(cancel_tx),
        })
    }

    async fn connect_async(
        session_id: u64,
        request: SshConnectRequest,
        sftp_sessions: Arc<Mutex<HashMap<u64, Arc<tokio::sync::Mutex<SftpSession>>>>>,
    ) -> std::result::Result<SshConnectResult, SshError> {
        let config = Arc::new(client::Config::default());
        let addr = (
            request.connection.hostname.as_str(),
            request.connection.port,
        );
        let mut session = client::connect(config, addr, AcceptAnyHostKeyHandler)
            .await
            .map_err(|err| SshError::Transport(err.to_string()))?;

        authenticate(
            &mut session,
            &request.connection.username,
            &request.auth_order,
        )
        .await?;

        let channel = session
            .channel_open_session()
            .await
            .map_err(|err| SshError::Transport(err.to_string()))?;

        let geometry = TerminalGeometry::default();
        channel
            .request_pty(
                true,
                "xterm-256color",
                u32::from(geometry.size.cols),
                u32::from(geometry.size.rows),
                u32::from(geometry.pixel_size.width_px),
                u32::from(geometry.pixel_size.height_px),
                &[],
            )
            .await
            .map_err(|err| SshError::Transport(err.to_string()))?;
        channel
            .request_shell(true)
            .await
            .map_err(|err| SshError::Transport(err.to_string()))?;

        let sftp = Self::bootstrap_sftp(session_id, &session, &sftp_sessions).await?;

        let (state, notify_rx) = SharedSessionState::new(format!(
            "Connected to {}@{}:{}",
            request.connection.username, request.connection.hostname, request.connection.port
        ));
        let (command_tx, command_rx) = mpsc::unbounded_channel::<SessionCommand>();
        let handle = Arc::new(SshSessionHandle::new(
            session_id,
            request.connection.label.clone(),
            state.clone(),
            command_tx,
            notify_rx,
        ));

        std::thread::Builder::new()
            .name(format!("seance-ssh-session-{session_id}"))
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to initialize SSH session runtime");
                runtime.block_on(run_ssh_session(
                    session, channel, state, geometry, command_rx,
                ));
            })
            .map_err(|err| SshError::Transport(err.to_string()))?;

        Ok(SshConnectResult {
            session: handle,
            sftp,
        })
    }

    async fn bootstrap_sftp(
        session_id: u64,
        session: &client::Handle<AcceptAnyHostKeyHandler>,
        sftp_sessions: &Arc<Mutex<HashMap<u64, Arc<tokio::sync::Mutex<SftpSession>>>>>,
    ) -> std::result::Result<SftpBootstrapHandle, SshError> {
        let channel = session
            .channel_open_session()
            .await
            .map_err(|err| SshError::SftpBootstrap(err.to_string()))?;
        channel
            .request_subsystem(true, "sftp")
            .await
            .map_err(|err| SshError::SftpBootstrap(err.to_string()))?;
        let sftp = SftpSession::new(channel.into_stream())
            .await
            .map_err(|err| SshError::SftpBootstrap(err.to_string()))?;
        sftp_sessions
            .lock()
            .expect("sftp session map poisoned")
            .insert(session_id, Arc::new(tokio::sync::Mutex::new(sftp)));

        Ok(SftpBootstrapHandle {
            session_id,
            ready: true,
        })
    }

    async fn run_connect_future<T, F>(
        session_id: u64,
        connect_future: F,
        mut cancel_rx: oneshot::Receiver<()>,
        timeout: Duration,
        sftp_sessions: &Arc<Mutex<HashMap<u64, Arc<tokio::sync::Mutex<SftpSession>>>>>,
    ) -> std::result::Result<T, SshError>
    where
        F: std::future::Future<Output = std::result::Result<T, SshError>>,
    {
        let timeout_future = tokio::time::timeout(timeout, connect_future);
        tokio::pin!(timeout_future);

        let result = tokio::select! {
            biased;
            cancel_result = &mut cancel_rx => {
                match cancel_result {
                    Ok(()) => {
                        debug!(session_id, "SSH connect cancelled");
                        Err(SshError::Cancelled)
                    }
                    Err(_) => {
                        trace!(session_id, "SSH connect cancellation handle dropped");
                        Err(SshError::Cancelled)
                    }
                }
            }
            result = &mut timeout_future => {
                match result {
                    Ok(result) => result,
                    Err(_) => {
                        warn!(session_id, timeout_secs = timeout.as_secs(), "SSH connect timed out");
                        Err(SshError::TimedOut)
                    }
                }
            }
        };

        if result.is_err() {
            Self::clear_sftp_session(session_id, sftp_sessions);
        }

        result
    }

    fn clear_sftp_session(
        session_id: u64,
        sftp_sessions: &Arc<Mutex<HashMap<u64, Arc<tokio::sync::Mutex<SftpSession>>>>>,
    ) {
        sftp_sessions
            .lock()
            .expect("sftp session map poisoned")
            .remove(&session_id);
    }

    pub(crate) fn get_sftp(
        &self,
        session_id: u64,
    ) -> std::result::Result<Arc<tokio::sync::Mutex<SftpSession>>, SshError> {
        self.sftp_sessions
            .lock()
            .expect("sftp session map poisoned")
            .get(&session_id)
            .cloned()
            .ok_or(SshError::SftpNotConnected)
    }
}

#[cfg(test)]
mod tests {
    use std::{future::pending, time::Duration};

    use tokio::sync::oneshot;

    use crate::model::{SshConnectRequest, SshConnectionConfig, SshError};

    use super::SshSessionManager;

    #[test]
    fn rejects_missing_auth_methods() {
        let manager = SshSessionManager::new().unwrap();
        let err = manager
            .connect(SshConnectRequest {
                connection: SshConnectionConfig {
                    label: "demo".into(),
                    hostname: "localhost".into(),
                    port: 22,
                    username: "demo".into(),
                },
                auth_order: Vec::new(),
            })
            .unwrap_err();

        assert!(matches!(err, SshError::MissingAuthMethods));
    }

    #[test]
    fn connect_wrapper_maps_timeout() {
        let manager = SshSessionManager::new().unwrap();
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        let sftp_sessions = manager.sftp_sessions.clone();
        let err = manager
            .runtime
            .block_on(SshSessionManager::run_connect_future(
                41,
                pending::<std::result::Result<(), SshError>>(),
                cancel_rx,
                Duration::from_millis(10),
                &sftp_sessions,
            ));

        assert!(matches!(err, Err(SshError::TimedOut)));
    }

    #[test]
    fn connect_wrapper_maps_cancel() {
        let manager = SshSessionManager::new().unwrap();
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let sftp_sessions = manager.sftp_sessions.clone();
        let err = manager.runtime.block_on(async {
            cancel_tx.send(()).unwrap();
            SshSessionManager::run_connect_future(
                42,
                pending::<std::result::Result<(), SshError>>(),
                cancel_rx,
                Duration::from_secs(1),
                &sftp_sessions,
            )
            .await
        });

        assert!(matches!(err, Err(SshError::Cancelled)));
    }
}
