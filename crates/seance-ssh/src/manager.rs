use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result};
use russh::client;
use russh_sftp::client::SftpSession;
use seance_terminal::{SharedSessionState, TerminalGeometry, next_session_id};
use tokio::{runtime::Runtime, sync::mpsc};

use crate::{
    auth::{AcceptAnyHostKeyHandler, authenticate},
    backend::run_ssh_session,
    model::{SftpBootstrapHandle, SshConnectRequest, SshConnectResult, SshError},
    session::{SessionCommand, SshSessionHandle},
};

pub struct SshSessionManager {
    pub(crate) runtime: Runtime,
    pub(crate) sftp_sessions: Arc<Mutex<HashMap<u64, Arc<tokio::sync::Mutex<SftpSession>>>>>,
}

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
        let session_id = next_session_id();
        self.runtime
            .block_on(self.connect_async(session_id, request))
    }

    pub fn sftp_ready(&self, session_id: u64) -> bool {
        self.sftp_sessions
            .lock()
            .expect("sftp session map poisoned")
            .contains_key(&session_id)
    }

    async fn connect_async(
        &self,
        session_id: u64,
        request: SshConnectRequest,
    ) -> std::result::Result<SshConnectResult, SshError> {
        if request.auth_order.is_empty() {
            return Err(SshError::MissingAuthMethods);
        }

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

        let sftp = self.bootstrap_sftp(session_id, &session).await?;

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
        &self,
        session_id: u64,
        session: &client::Handle<AcceptAnyHostKeyHandler>,
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
        self.sftp_sessions
            .lock()
            .expect("sftp session map poisoned")
            .insert(session_id, Arc::new(tokio::sync::Mutex::new(sftp)));

        Ok(SftpBootstrapHandle {
            session_id,
            ready: true,
        })
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
}
