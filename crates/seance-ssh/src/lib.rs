use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::{Context, Result};
use russh::keys::{Algorithm as SshAlgorithm, PrivateKey as SshPrivateKey, PublicKey};
use russh::{
    Channel, ChannelMsg, ChannelReadHalf, ChannelWriteHalf, client, keys::PrivateKeyWithHashAlg,
};
use russh_sftp::client::SftpSession;
use seance_terminal::{SharedSessionState, TerminalEmulator, TerminalGeometry, TerminalSession};
use thiserror::Error;
use tokio::{io::AsyncWriteExt, runtime::Runtime, sync::mpsc};

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
}

#[derive(Default)]
struct AcceptAnyHostKeyHandler;

impl client::Handler for AcceptAnyHostKeyHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        Ok(true)
    }
}

#[derive(Debug)]
enum SessionCommand {
    Input(Vec<u8>),
    Resize(TerminalGeometry),
}

pub struct SshSessionHandle {
    id: u64,
    title: Arc<str>,
    state: SharedSessionState,
    command_tx: mpsc::UnboundedSender<SessionCommand>,
    notify_rx: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}

impl std::fmt::Debug for SshSessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshSessionHandle")
            .field("id", &self.id)
            .field("title", &self.title)
            .finish()
    }
}

impl TerminalSession for SshSessionHandle {
    fn id(&self) -> u64 {
        self.id
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn snapshot(&self) -> seance_terminal::SessionSnapshot {
        self.state.snapshot()
    }

    fn send_input(&self, bytes: Vec<u8>) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Input(bytes))
            .map_err(|_| anyhow::anyhow!("failed to forward input to SSH session"))
    }

    fn resize(&self, geometry: TerminalGeometry) -> Result<()> {
        self.command_tx
            .send(SessionCommand::Resize(geometry))
            .map_err(|_| anyhow::anyhow!("failed to forward resize to SSH session"))
    }

    fn perf_snapshot(&self) -> seance_terminal::SessionPerfSnapshot {
        self.state.perf_snapshot()
    }

    fn take_notify_rx(&self) -> Option<std::sync::mpsc::Receiver<()>> {
        self.notify_rx.lock().expect("notify_rx poisoned").take()
    }
}

pub struct SshSessionManager {
    runtime: Runtime,
    next_id: AtomicU64,
    sftp_sessions: Arc<Mutex<HashMap<u64, Arc<tokio::sync::Mutex<SftpSession>>>>>,
}

impl SshSessionManager {
    pub fn new() -> Result<Self> {
        Ok(Self {
            runtime: tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .context("failed to initialize SSH runtime")?,
            next_id: AtomicU64::new(1),
            sftp_sessions: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    pub fn connect(
        &self,
        request: SshConnectRequest,
    ) -> std::result::Result<SshConnectResult, SshError> {
        let session_id = self.next_id.fetch_add(1, Ordering::Relaxed);
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
        let (command_tx, command_rx) = mpsc::unbounded_channel();
        let handle = Arc::new(SshSessionHandle {
            id: session_id,
            title: Arc::<str>::from(request.connection.label.clone()),
            state: state.clone(),
            command_tx,
            notify_rx: Mutex::new(Some(notify_rx)),
        });

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
}

async fn authenticate(
    session: &mut client::Handle<AcceptAnyHostKeyHandler>,
    username: &str,
    auth_order: &[ResolvedAuthMethod],
) -> std::result::Result<(), SshError> {
    for auth in auth_order {
        let result = match auth {
            ResolvedAuthMethod::Password { password } => session
                .authenticate_password(username, password.clone())
                .await
                .map_err(|err| SshError::Transport(err.to_string()))?,
            ResolvedAuthMethod::PrivateKey {
                private_key_pem,
                passphrase,
            } => {
                let mut private_key = SshPrivateKey::from_openssh(private_key_pem)
                    .map_err(|err| SshError::InvalidPrivateKey(err.to_string()))?;
                if private_key.is_encrypted() {
                    let Some(passphrase) = passphrase.as_ref() else {
                        return Err(SshError::InvalidPrivateKey(
                            "encrypted private key is missing a passphrase".into(),
                        ));
                    };
                    private_key = private_key
                        .decrypt(passphrase)
                        .map_err(|err| SshError::InvalidPrivateKey(err.to_string()))?;
                }
                let hash_alg = match private_key.algorithm() {
                    SshAlgorithm::Rsa { .. } => session
                        .best_supported_rsa_hash()
                        .await
                        .map_err(|err| SshError::Transport(err.to_string()))?
                        .flatten(),
                    _ => None,
                };
                session
                    .authenticate_publickey(
                        username,
                        PrivateKeyWithHashAlg::new(Arc::new(private_key), hash_alg),
                    )
                    .await
                    .map_err(|err| SshError::Transport(err.to_string()))?
            }
        };

        if result.success() {
            return Ok(());
        }
    }

    Err(SshError::AuthenticationRejected)
}

async fn run_ssh_session(
    _session: client::Handle<AcceptAnyHostKeyHandler>,
    channel: Channel<russh::client::Msg>,
    state: SharedSessionState,
    geometry: TerminalGeometry,
    mut command_rx: mpsc::UnboundedReceiver<SessionCommand>,
) {
    let mut emulator = match TerminalEmulator::new(geometry) {
        Ok(emulator) => emulator,
        Err(error) => {
            state.set_error(&error);
            return;
        }
    };
    emulator.publish(&state, None);

    let (mut read_half, write_half): (ChannelReadHalf, ChannelWriteHalf<russh::client::Msg>) =
        channel.split();
    let mut writer = write_half.make_writer();
    let mut exit_status = None;

    loop {
        tokio::select! {
            Some(command) = command_rx.recv() => {
                match command {
                    SessionCommand::Input(bytes) => {
                        if let Err(error) = writer.write_all(&bytes).await {
                            state.set_error(&anyhow::anyhow!("failed to write to SSH channel: {error}"));
                            break;
                        }
                        let _ = writer.flush().await;
                    }
                    SessionCommand::Resize(geometry) => {
                        let _ = write_half
                            .window_change(
                                u32::from(geometry.size.cols),
                                u32::from(geometry.size.rows),
                                u32::from(geometry.pixel_size.width_px),
                                u32::from(geometry.pixel_size.height_px),
                            )
                            .await;
                        let _ = emulator.resize(geometry);
                    }
                }
            }
            msg = read_half.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        emulator.write(&data);
                        emulator.publish(&state, exit_status.clone());
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        emulator.write(&data);
                        emulator.publish(&state, exit_status.clone());
                    }
                    Some(ChannelMsg::ExitStatus { exit_status: code }) => {
                        exit_status = Some(format!("remote exited with status {code}"));
                        emulator.publish(&state, exit_status.clone());
                    }
                    Some(ChannelMsg::ExitSignal { signal_name, .. }) => {
                        exit_status = Some(format!("remote exited via signal {signal_name:?}"));
                        emulator.publish(&state, exit_status.clone());
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => {
                        emulator.publish(
                            &state,
                            Some(exit_status.unwrap_or_else(|| "remote session closed".into())),
                        );
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
