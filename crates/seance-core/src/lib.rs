use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use anyhow::{Context, Result, anyhow};
use seance_ssh::{
    ResolvedAuthMethod, SftpEntry, SshConnectRequest, SshConnectionConfig, SshSessionManager,
};
use seance_terminal::{LocalSessionFactory, TerminalSession};
use seance_vault::{
    CredentialSummary, GenerateKeyRequest, HostAuthRef, HostSummary, KeySummary, SecretString,
    VaultHostProfile, VaultPasswordCredential, VaultStatus, VaultStore,
};

pub type SessionId = u64;

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub app_root: PathBuf,
    pub vault_db_path: PathBuf,
    pub ipc_socket_path: PathBuf,
    pub instance_lock_path: PathBuf,
}

impl AppPaths {
    pub fn detect() -> Result<Self> {
        let data_root = dirs::data_local_dir().unwrap_or(std::env::current_dir()?);
        let app_root = data_root.join("seance");
        fs::create_dir_all(&app_root).context("failed to create app data directory")?;
        Ok(Self {
            vault_db_path: app_root.join("vault.sqlite"),
            ipc_socket_path: app_root.join("resident.sock"),
            instance_lock_path: app_root.join("resident.lock"),
            app_root,
        })
    }
}

pub struct AppContext {
    pub paths: AppPaths,
    pub vault: VaultStore,
    pub ssh: Arc<SshSessionManager>,
    pub local: LocalSessionFactory,
}

impl AppContext {
    pub fn open(paths: AppPaths) -> Result<Self> {
        let vault =
            VaultStore::open(&paths.vault_db_path).context("failed to open the encrypted vault")?;
        let ssh = Arc::new(SshSessionManager::new()?);
        Ok(Self {
            paths,
            vault,
            ssh,
            local: LocalSessionFactory::default(),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKind {
    Local,
    Remote,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowTarget {
    MostRecentOrNew,
    NewLocal,
    Session { session_id: SessionId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlatformCloseAction {
    Hide,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LifecyclePolicy {
    pub keep_running_without_windows: bool,
    pub hide_on_last_window_close: bool,
    pub keep_sessions_alive_without_windows: bool,
}

impl Default for LifecyclePolicy {
    fn default() -> Self {
        Self {
            keep_running_without_windows: true,
            hide_on_last_window_close: true,
            keep_sessions_alive_without_windows: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WindowBootstrap {
    pub attached_session_id: SessionId,
    pub saved_hosts: Vec<HostSummary>,
    pub cached_credentials: Vec<CredentialSummary>,
    pub cached_keys: Vec<KeySummary>,
    pub vault_status: VaultStatus,
    pub device_unlock_attempted: bool,
}

#[derive(Default)]
pub struct SessionRegistry {
    sessions: HashMap<SessionId, Arc<dyn TerminalSession>>,
    metadata: HashMap<SessionId, SessionMetadata>,
}

#[derive(Clone, Copy, Debug)]
struct SessionMetadata {
    kind: SessionKind,
    last_access_seq: u64,
}

impl SessionRegistry {
    fn insert(&mut self, session: Arc<dyn TerminalSession>, kind: SessionKind, seq: u64) {
        let id = session.id();
        self.sessions.insert(id, session);
        self.metadata.insert(
            id,
            SessionMetadata {
                kind,
                last_access_seq: seq,
            },
        );
    }

    fn get(&self, id: SessionId) -> Option<Arc<dyn TerminalSession>> {
        self.sessions.get(&id).cloned()
    }

    fn list(&self) -> Vec<Arc<dyn TerminalSession>> {
        let mut sessions = self.sessions.values().cloned().collect::<Vec<_>>();
        sessions.sort_by_key(|session| session.id());
        sessions
    }

    fn kind(&self, id: SessionId) -> Option<SessionKind> {
        self.metadata.get(&id).map(|meta| meta.kind)
    }

    fn touch(&mut self, id: SessionId, seq: u64) {
        if let Some(meta) = self.metadata.get_mut(&id) {
            meta.last_access_seq = seq;
        }
    }

    fn remove(&mut self, id: SessionId) -> bool {
        let removed = self.sessions.remove(&id).is_some();
        self.metadata.remove(&id);
        removed
    }

    fn most_recent_session_id(&self) -> Option<SessionId> {
        self.metadata
            .iter()
            .max_by_key(|(_, meta)| meta.last_access_seq)
            .map(|(id, _)| *id)
    }

    fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}

#[derive(Default)]
pub struct WindowRegistry {
    open_windows: usize,
}

impl WindowRegistry {
    fn open_window(&mut self) {
        self.open_windows += 1;
    }

    fn close_window(&mut self) {
        self.open_windows = self.open_windows.saturating_sub(1);
    }

    fn open_count(&self) -> usize {
        self.open_windows
    }
}

pub struct AppController {
    context: AppContext,
    sessions: SessionRegistry,
    windows: WindowRegistry,
    lifecycle_policy: LifecyclePolicy,
    access_seq: u64,
    device_unlock_attempted: bool,
}

#[derive(Clone)]
pub struct AppControllerHandle(Arc<Mutex<AppController>>);

impl AppControllerHandle {
    pub fn new(context: AppContext) -> Self {
        Self(Arc::new(Mutex::new(AppController {
            context,
            sessions: SessionRegistry::default(),
            windows: WindowRegistry::default(),
            lifecycle_policy: LifecyclePolicy::default(),
            access_seq: 0,
            device_unlock_attempted: false,
        })))
    }

    fn with_lock<R>(&self, f: impl FnOnce(&mut AppController) -> R) -> R {
        let mut guard = self.0.lock().expect("app controller mutex poisoned");
        f(&mut guard)
    }

    pub fn app_paths(&self) -> AppPaths {
        self.with_lock(|controller| controller.context.paths.clone())
    }

    pub fn bootstrap(&self) -> Result<()> {
        self.with_lock(|controller| controller.bootstrap())
    }

    pub fn prepare_window(&self, target: WindowTarget) -> Result<WindowBootstrap> {
        self.with_lock(|controller| controller.prepare_window(target))
    }

    pub fn on_window_opened(&self) {
        self.with_lock(|controller| controller.windows.open_window());
    }

    pub fn on_window_closed(&self) {
        self.with_lock(|controller| controller.windows.close_window());
    }

    pub fn on_last_window_closed(&self) -> PlatformCloseAction {
        self.with_lock(|controller| {
            if controller.lifecycle_policy.keep_running_without_windows
                && controller.lifecycle_policy.hide_on_last_window_close
            {
                PlatformCloseAction::Hide
            } else {
                PlatformCloseAction::Exit
            }
        })
    }

    pub fn open_window_count(&self) -> usize {
        self.with_lock(|controller| controller.windows.open_count())
    }

    pub fn list_sessions(&self) -> Vec<Arc<dyn TerminalSession>> {
        self.with_lock(|controller| controller.sessions.list())
    }

    pub fn session_kind(&self, id: SessionId) -> Option<SessionKind> {
        self.with_lock(|controller| controller.sessions.kind(id))
    }

    pub fn get_session(&self, id: SessionId) -> Option<Arc<dyn TerminalSession>> {
        self.with_lock(|controller| controller.sessions.get(id))
    }

    pub fn most_recent_session_id(&self) -> Option<SessionId> {
        self.with_lock(|controller| controller.sessions.most_recent_session_id())
    }

    pub fn touch_session(&self, id: SessionId) {
        self.with_lock(|controller| {
            controller.bump_access_seq();
            let seq = controller.access_seq;
            controller.sessions.touch(id, seq);
        });
    }

    pub fn spawn_local_session(&self) -> Result<Arc<dyn TerminalSession>> {
        self.with_lock(|controller| controller.spawn_local_session())
    }

    pub fn register_remote_session(&self, session: Arc<dyn TerminalSession>) {
        self.with_lock(|controller| controller.register_session(session, SessionKind::Remote));
    }

    pub fn close_session(&self, id: SessionId) -> bool {
        self.with_lock(|controller| controller.sessions.remove(id))
    }

    pub fn ssh_manager(&self) -> Arc<SshSessionManager> {
        self.with_lock(|controller| Arc::clone(&controller.context.ssh))
    }

    pub fn vault_status(&self) -> VaultStatus {
        self.with_lock(|controller| controller.context.vault.status())
    }

    pub fn try_unlock_with_device(&self) -> Result<bool> {
        self.with_lock(|controller| {
            controller.device_unlock_attempted = true;
            Ok(controller.context.vault.try_unlock_with_device()?)
        })
    }

    pub fn create_vault(&self, passphrase: &SecretString, device_name: &str) -> Result<()> {
        self.with_lock(|controller| {
            Ok(controller
                .context
                .vault
                .create_vault(passphrase, device_name)?)
        })
    }

    pub fn unlock_vault(&self, passphrase: &SecretString, device_name: &str) -> Result<()> {
        self.with_lock(|controller| {
            Ok(controller
                .context
                .vault
                .unlock_with_passphrase(passphrase, device_name)?)
        })
    }

    pub fn lock_vault(&self) {
        self.with_lock(|controller| controller.context.vault.lock());
    }

    pub fn list_hosts(&self) -> Result<Vec<HostSummary>> {
        self.with_lock(|controller| Ok(controller.context.vault.list_host_profiles()?))
    }

    pub fn load_host(&self, id: &str) -> Result<Option<VaultHostProfile>> {
        self.with_lock(|controller| Ok(controller.context.vault.load_host_profile(id)?))
    }

    pub fn save_host(&self, host: VaultHostProfile) -> Result<HostSummary> {
        self.with_lock(|controller| Ok(controller.context.vault.store_host_profile(host)?))
    }

    pub fn delete_host(&self, id: &str) -> Result<bool> {
        self.with_lock(|controller| Ok(controller.context.vault.delete_host_profile(id)?))
    }

    pub fn list_password_credentials(&self) -> Result<Vec<CredentialSummary>> {
        self.with_lock(|controller| Ok(controller.context.vault.list_password_credentials()?))
    }

    pub fn load_password_credential(&self, id: &str) -> Result<Option<VaultPasswordCredential>> {
        self.with_lock(|controller| Ok(controller.context.vault.load_password_credential(id)?))
    }

    pub fn save_password_credential(
        &self,
        credential: VaultPasswordCredential,
    ) -> Result<CredentialSummary> {
        self.with_lock(|controller| {
            Ok(controller
                .context
                .vault
                .store_password_credential(credential)?)
        })
    }

    pub fn delete_password_credential(&self, id: &str) -> Result<bool> {
        self.with_lock(|controller| Ok(controller.context.vault.delete_password_credential(id)?))
    }

    pub fn list_private_keys(&self) -> Result<Vec<KeySummary>> {
        self.with_lock(|controller| Ok(controller.context.vault.list_private_keys()?))
    }

    pub fn generate_private_key(&self, request: GenerateKeyRequest) -> Result<KeySummary> {
        self.with_lock(|controller| Ok(controller.context.vault.generate_private_key(request)?))
    }

    pub fn delete_private_key(&self, id: &str) -> Result<bool> {
        self.with_lock(|controller| Ok(controller.context.vault.delete_private_key(id)?))
    }

    pub fn build_connect_request(&self, host_id: &str) -> Result<SshConnectRequest> {
        self.with_lock(|controller| {
            let host = controller
                .context
                .vault
                .load_host_profile(host_id)?
                .ok_or_else(|| anyhow!("saved host not found"))?;

            let mut auth_order = Vec::with_capacity(host.auth_order.len());
            for auth in &host.auth_order {
                match auth {
                    HostAuthRef::Password { credential_id } => {
                        let credential = controller
                            .context
                            .vault
                            .load_password_credential(credential_id)?
                            .ok_or_else(|| anyhow!("missing password credential"))?;
                        auth_order.push(ResolvedAuthMethod::Password {
                            password: credential.secret,
                        });
                    }
                    HostAuthRef::PrivateKey {
                        key_id,
                        passphrase_credential_id,
                    } => {
                        let key = controller
                            .context
                            .vault
                            .load_private_key(key_id)?
                            .ok_or_else(|| anyhow!("missing private key"))?;
                        let passphrase = passphrase_credential_id
                            .as_ref()
                            .map(|id| controller.context.vault.load_password_credential(id))
                            .transpose()?
                            .flatten()
                            .map(|credential| credential.secret);
                        auth_order.push(ResolvedAuthMethod::PrivateKey {
                            private_key_pem: key.private_key_pem,
                            passphrase,
                        });
                    }
                }
            }

            Ok(SshConnectRequest {
                connection: SshConnectionConfig {
                    label: host.label,
                    hostname: host.hostname,
                    port: host.port,
                    username: host.username,
                },
                auth_order,
            })
        })
    }

    pub fn sftp_canonicalize(&self, session_id: SessionId, path: &str) -> Result<String> {
        self.with_lock(|controller| {
            Ok(controller.context.ssh.sftp_canonicalize(session_id, path)?)
        })
    }

    pub fn sftp_list_dir(&self, session_id: SessionId, path: &str) -> Result<Vec<SftpEntry>> {
        self.with_lock(|controller| Ok(controller.context.ssh.sftp_list_dir(session_id, path)?))
    }

    pub fn sftp_read_file(&self, session_id: SessionId, remote_path: &str) -> Result<Vec<u8>> {
        self.with_lock(|controller| {
            Ok(controller
                .context
                .ssh
                .sftp_read_file(session_id, remote_path)?)
        })
    }

    pub fn sftp_write_file(
        &self,
        session_id: SessionId,
        remote_path: &str,
        data: &[u8],
    ) -> Result<()> {
        self.with_lock(|controller| {
            Ok(controller
                .context
                .ssh
                .sftp_write_file(session_id, remote_path, data)?)
        })
    }

    pub fn sftp_mkdir(&self, session_id: SessionId, path: &str) -> Result<()> {
        self.with_lock(|controller| Ok(controller.context.ssh.sftp_mkdir(session_id, path)?))
    }

    pub fn sftp_remove(&self, session_id: SessionId, path: &str, is_dir: bool) -> Result<()> {
        self.with_lock(|controller| {
            Ok(controller
                .context
                .ssh
                .sftp_remove(session_id, path, is_dir)?)
        })
    }

    pub fn sftp_rename(&self, session_id: SessionId, old_path: &str, new_path: &str) -> Result<()> {
        self.with_lock(|controller| {
            Ok(controller
                .context
                .ssh
                .sftp_rename(session_id, old_path, new_path)?)
        })
    }
}

impl AppController {
    fn bootstrap(&mut self) -> Result<()> {
        if self.context.vault.status().initialized {
            let _ = self.context.vault.try_unlock_with_device();
            self.device_unlock_attempted = true;
        }
        if self.sessions.is_empty() {
            let _ = self.spawn_local_session()?;
        }
        Ok(())
    }

    fn prepare_window(&mut self, target: WindowTarget) -> Result<WindowBootstrap> {
        let attached_session_id = match target {
            WindowTarget::MostRecentOrNew => self
                .sessions
                .most_recent_session_id()
                .or_else(|| self.spawn_local_session().ok().map(|session| session.id()))
                .ok_or_else(|| anyhow!("failed to obtain session for window"))?,
            WindowTarget::NewLocal => self.spawn_local_session()?.id(),
            WindowTarget::Session { session_id } => self
                .sessions
                .get(session_id)
                .map(|session| session.id())
                .ok_or_else(|| anyhow!("requested session is no longer available"))?,
        };
        self.bump_access_seq();
        let seq = self.access_seq;
        self.sessions.touch(attached_session_id, seq);
        let unlocked = self.context.vault.status().unlocked;
        Ok(WindowBootstrap {
            attached_session_id,
            saved_hosts: if unlocked {
                self.context.vault.list_host_profiles().unwrap_or_default()
            } else {
                Vec::new()
            },
            cached_credentials: if unlocked {
                self.context
                    .vault
                    .list_password_credentials()
                    .unwrap_or_default()
            } else {
                Vec::new()
            },
            cached_keys: if unlocked {
                self.context.vault.list_private_keys().unwrap_or_default()
            } else {
                Vec::new()
            },
            vault_status: self.context.vault.status(),
            device_unlock_attempted: self.device_unlock_attempted,
        })
    }

    fn spawn_local_session(&mut self) -> Result<Arc<dyn TerminalSession>> {
        let session: Arc<dyn TerminalSession> = Arc::new(self.context.local.spawn()?);
        self.register_session(Arc::clone(&session), SessionKind::Local);
        Ok(session)
    }

    fn register_session(&mut self, session: Arc<dyn TerminalSession>, kind: SessionKind) {
        self.bump_access_seq();
        let seq = self.access_seq;
        self.sessions.insert(session, kind, seq);
    }

    fn bump_access_seq(&mut self) {
        self.access_seq = self.access_seq.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, mpsc};

    use anyhow::Result;
    use seance_terminal::{
        SessionPerfSnapshot, SessionSnapshot, TerminalGeometry, TerminalSession,
    };

    use super::{LifecyclePolicy, PlatformCloseAction, SessionKind, SessionRegistry};

    struct FakeSession(u64);

    impl TerminalSession for FakeSession {
        fn id(&self) -> u64 {
            self.0
        }
        fn title(&self) -> &str {
            "fake"
        }
        fn snapshot(&self) -> SessionSnapshot {
            SessionSnapshot::default()
        }
        fn send_input(&self, _bytes: Vec<u8>) -> Result<()> {
            Ok(())
        }
        fn resize(&self, _geometry: TerminalGeometry) -> Result<()> {
            Ok(())
        }
        fn perf_snapshot(&self) -> SessionPerfSnapshot {
            SessionPerfSnapshot::default()
        }
        fn take_notify_rx(&self) -> Option<mpsc::Receiver<()>> {
            None
        }
    }

    #[test]
    fn session_registry_tracks_recent_session() {
        let mut registry = SessionRegistry::default();
        registry.insert(Arc::new(FakeSession(1)), SessionKind::Local, 1);
        registry.insert(Arc::new(FakeSession(2)), SessionKind::Remote, 2);
        assert_eq!(registry.most_recent_session_id(), Some(2));
        registry.touch(1, 3);
        assert_eq!(registry.most_recent_session_id(), Some(1));
    }

    #[test]
    fn lifecycle_defaults_hide_on_last_window_close() {
        let policy = LifecyclePolicy::default();
        let action = if policy.keep_running_without_windows && policy.hide_on_last_window_close {
            PlatformCloseAction::Hide
        } else {
            PlatformCloseAction::Exit
        };
        assert_eq!(action, PlatformCloseAction::Hide);
    }
}
