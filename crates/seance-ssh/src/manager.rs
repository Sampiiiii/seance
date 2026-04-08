use std::{
    collections::HashMap,
    sync::{Arc, Mutex, mpsc::Receiver},
    time::Duration,
};

use anyhow::{Context, Result};
use russh::client;
use russh_sftp::client::SftpSession;
use seance_terminal::{
    SharedSessionState, TerminalGeometry, TerminalTranscriptSink, next_session_id,
};
use tokio::{
    runtime::Runtime,
    sync::{mpsc, oneshot},
};
use tracing::{debug, trace, warn};

use crate::{
    auth::{SshClientHandler, authenticate},
    backend::run_ssh_session,
    model::{
        PortForwardRuntimeSnapshot, SftpBootstrapHandle, SshConnectAbortHandle, SshConnectRequest,
        SshConnectResult, SshConnectTask, SshError, SshPortForwardRequest,
    },
    session::{SessionCommand, SshSessionHandle},
    tunnel::{TunnelRegistry, run_port_forward},
};

type SharedSftpSession = Arc<tokio::sync::Mutex<SftpSession>>;
type SftpSessionMap = Arc<Mutex<HashMap<u64, SharedSftpSession>>>;

pub struct SshSessionManager {
    pub(crate) runtime: Runtime,
    pub(crate) sftp_sessions: SftpSessionMap,
    pub(crate) tunnel_registry: Arc<TunnelRegistry>,
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
            tunnel_registry: Arc::new(TunnelRegistry::new()),
        })
    }

    pub fn connect(
        &self,
        request: SshConnectRequest,
        transcript_sink: Arc<dyn TerminalTranscriptSink>,
    ) -> std::result::Result<SshConnectResult, SshError> {
        if request.auth_order.is_empty() {
            return Err(SshError::MissingAuthMethods);
        }

        let session_id = next_session_id();
        let sftp_sessions = Arc::clone(&self.sftp_sessions);
        let (_cancel_tx, cancel_rx) = oneshot::channel();
        self.runtime.block_on(Self::run_connect_future(
            session_id,
            Self::connect_async(
                session_id,
                request,
                Arc::clone(&sftp_sessions),
                transcript_sink,
            ),
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
        transcript_sink: Arc<dyn TerminalTranscriptSink>,
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
        std::mem::drop(self.runtime.spawn(async move {
            let result = Self::run_connect_future(
                session_id,
                Self::connect_async(
                    session_id,
                    request,
                    Arc::clone(&sftp_sessions),
                    transcript_sink,
                ),
                cancel_rx,
                CONNECT_TIMEOUT,
                &sftp_sessions,
            )
            .await;
            let _ = result_tx.send(result);
        }));

        Ok(SshConnectTask {
            session_id,
            result_rx,
            abort_handle: SshConnectAbortHandle::new(cancel_tx),
        })
    }

    pub fn start_port_forward(
        &self,
        request: SshPortForwardRequest,
    ) -> std::result::Result<PortForwardRuntimeSnapshot, SshError> {
        if request.auth_order.is_empty() {
            return Err(SshError::MissingAuthMethods);
        }
        if self.tunnel_registry.has_active_handle(&request.id) {
            return Err(SshError::PortForwardAlreadyRunning(request.id));
        }

        let snapshot = self.tunnel_registry.upsert_starting(&request);
        let (cancel_tx, cancel_rx) = oneshot::channel();
        self.tunnel_registry.insert_handle(
            request.id.clone(),
            crate::model::SshPortForwardHandle::new(cancel_tx),
        )?;

        let registry = Arc::clone(&self.tunnel_registry);
        let id = request.id.clone();
        std::mem::drop(self.runtime.spawn(async move {
            let result = run_port_forward(request, Arc::clone(&registry), cancel_rx).await;
            registry.remove_handle(&id);
            match result {
                Ok(()) => registry.remove_snapshot(&id),
                Err(SshError::Cancelled) => registry.remove_snapshot(&id),
                Err(error) => registry.mark_failed(&id, error.to_string()),
            }
        }));

        Ok(snapshot)
    }

    pub fn stop_port_forward(&self, id: &str) -> bool {
        self.tunnel_registry.stop(id)
    }

    pub fn list_port_forwards(&self) -> Vec<PortForwardRuntimeSnapshot> {
        self.tunnel_registry.list()
    }

    pub fn subscribe_tunnel_state_changes(&self) -> Receiver<Vec<PortForwardRuntimeSnapshot>> {
        self.tunnel_registry.subscribe()
    }

    async fn connect_async(
        session_id: u64,
        request: SshConnectRequest,
        sftp_sessions: SftpSessionMap,
        transcript_sink: Arc<dyn TerminalTranscriptSink>,
    ) -> std::result::Result<SshConnectResult, SshError> {
        let config = Arc::new(client::Config::default());
        let addr = (
            request.connection.hostname.as_str(),
            request.connection.port,
        );
        let mut session = client::connect(config, addr, SshClientHandler::default())
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

        let (state, notify_rx) = SharedSessionState::new(
            format!(
                "Connected to {}@{}:{}",
                request.connection.username, request.connection.hostname, request.connection.port
            ),
            geometry,
        );
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
                    session,
                    channel,
                    state,
                    geometry,
                    command_rx,
                    transcript_sink,
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
        session: &client::Handle<SshClientHandler>,
        sftp_sessions: &SftpSessionMap,
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
        sftp_sessions: &SftpSessionMap,
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

    fn clear_sftp_session(session_id: u64, sftp_sessions: &SftpSessionMap) {
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

    use seance_terminal::NoopTranscriptSink;
    use tokio::sync::oneshot;

    use crate::model::{SshConnectRequest, SshConnectionConfig, SshError};

    use super::SshSessionManager;

    #[test]
    fn rejects_missing_auth_methods() {
        let manager = SshSessionManager::new().unwrap();
        let err = manager
            .connect(
                SshConnectRequest {
                    connection: SshConnectionConfig {
                        label: "demo".into(),
                        hostname: "localhost".into(),
                        port: 22,
                        username: "demo".into(),
                    },
                    auth_order: Vec::new(),
                },
                std::sync::Arc::new(NoopTranscriptSink),
            )
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
