use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
};

use anyhow::{Context, Result, anyhow};
use seance_config::{AppConfig, ConfigStore, VaultRegistryEntry};
use seance_ssh::{
    ResolvedAuthMethod, SftpEntry, SshConnectRequest, SshConnectionConfig, SshSessionManager,
};
use seance_terminal::{LocalSessionFactory, TerminalSession};
use seance_updater::UpdateManager;
pub use seance_updater::{InstallMode, ReleaseChannel, UpdateInfo, UpdateSettings, UpdateState};
use seance_vault::{
    CredentialSummary, GenerateKeyRequest, HostAuthRef, HostSummary, KeySummary, SecretString,
    VaultHostProfile, VaultPasswordCredential, VaultStatus, VaultStore,
};
use uuid::Uuid;

pub type SessionId = u64;
const LEGACY_VAULT_DB_FILE: &str = "vault.sqlite";

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub app_root: PathBuf,
    pub config_path: PathBuf,
    pub vault_db_path: PathBuf,
    pub vaults_dir: PathBuf,
    pub ipc_socket_path: PathBuf,
    pub instance_lock_path: PathBuf,
}

impl AppPaths {
    pub fn detect() -> Result<Self> {
        let data_root = dirs::data_local_dir().unwrap_or(std::env::current_dir()?);
        let app_root = data_root.join("seance");
        fs::create_dir_all(&app_root).context("failed to create app data directory")?;
        let vaults_dir = app_root.join("vaults");
        fs::create_dir_all(&vaults_dir).context("failed to create vault storage directory")?;
        Ok(Self {
            config_path: app_root.join("config.toml"),
            vault_db_path: app_root.join("vault.sqlite"),
            vaults_dir,
            ipc_socket_path: app_root.join("resident.sock"),
            instance_lock_path: app_root.join("resident.lock"),
            app_root,
        })
    }
}

pub struct AppContext {
    pub paths: AppPaths,
    pub config: ConfigStore,
    vaults: HashMap<String, ManagedVaultState>,
    pub ssh: Arc<SshSessionManager>,
    pub local: LocalSessionFactory,
    pub updater: Arc<UpdateManager>,
}

impl AppContext {
    pub fn open(paths: AppPaths) -> Result<Self> {
        let mut config = match ConfigStore::load_or_default(&paths.config_path) {
            Ok(config) => config,
            Err(error) => {
                tracing::warn!(
                    path = %paths.config_path.display(),
                    error = %error,
                    "failed to load config, falling back to defaults"
                );
                ConfigStore::with_defaults(&paths.config_path)
            }
        };
        migrate_legacy_vault_registry(&paths, &mut config)?;
        let vaults = load_managed_vaults(&paths, &config.snapshot())?;
        let ssh = Arc::new(SshSessionManager::new()?);
        let updater = Arc::new(UpdateManager::new(update_settings_from_config(
            &config.snapshot(),
        )));
        Ok(Self {
            paths,
            config,
            vaults,
            ssh,
            local: LocalSessionFactory,
            updater,
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
        Self::from(&AppConfig::default())
    }
}

impl From<&AppConfig> for LifecyclePolicy {
    fn from(config: &AppConfig) -> Self {
        Self {
            keep_running_without_windows: config.window.keep_running_without_windows,
            hide_on_last_window_close: config.window.hide_on_last_window_close,
            keep_sessions_alive_without_windows: config.window.keep_sessions_alive_without_windows,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WindowBootstrap {
    pub attached_session_id: SessionId,
    pub managed_vaults: Vec<ManagedVaultSummary>,
    pub saved_hosts: Vec<VaultScopedHostSummary>,
    pub cached_credentials: Vec<VaultScopedCredentialSummary>,
    pub cached_keys: Vec<VaultScopedKeySummary>,
    pub device_unlock_attempted: bool,
    pub config: AppConfig,
    pub update_state: UpdateState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedVaultSummary {
    pub vault_id: String,
    pub name: String,
    pub db_path: PathBuf,
    pub open: bool,
    pub initialized: bool,
    pub unlocked: bool,
    pub device_unlock_available: bool,
    pub device_unlock_message: Option<String>,
    pub host_count: usize,
    pub credential_count: usize,
    pub key_count: usize,
    pub availability_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultScopedHostSummary {
    pub vault_id: String,
    pub vault_name: String,
    pub host: HostSummary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultScopedCredentialSummary {
    pub vault_id: String,
    pub vault_name: String,
    pub credential: CredentialSummary,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultScopedKeySummary {
    pub vault_id: String,
    pub vault_name: String,
    pub key: KeySummary,
}

struct ManagedVaultState {
    entry: VaultRegistryEntry,
    store: Option<VaultStore>,
    availability_error: Option<String>,
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
    config_subscribers: Vec<Sender<AppConfig>>,
    access_seq: u64,
    device_unlock_attempted: bool,
}

#[derive(Clone)]
pub struct AppControllerHandle(Arc<Mutex<AppController>>);

impl AppControllerHandle {
    pub fn new(context: AppContext) -> Self {
        let lifecycle_policy = LifecyclePolicy::from(&context.config.snapshot());
        Self(Arc::new(Mutex::new(AppController {
            context,
            sessions: SessionRegistry::default(),
            windows: WindowRegistry::default(),
            lifecycle_policy,
            config_subscribers: Vec::new(),
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

    pub fn config_snapshot(&self) -> AppConfig {
        self.with_lock(|controller| controller.context.config.snapshot())
    }

    pub fn update_config(&self, f: impl FnOnce(&mut AppConfig)) -> Result<AppConfig> {
        self.with_lock(|controller| controller.update_config(f))
    }

    pub fn reset_config_to_defaults(&self) -> Result<AppConfig> {
        self.with_lock(|controller| controller.reset_config_to_defaults())
    }

    pub fn subscribe_config_changes(&self) -> Receiver<AppConfig> {
        self.with_lock(|controller| controller.subscribe_config_changes())
    }

    pub fn update_state_snapshot(&self) -> UpdateState {
        self.with_lock(|controller| controller.context.updater.state_snapshot())
    }

    pub fn subscribe_update_changes(&self) -> Receiver<UpdateState> {
        self.with_lock(|controller| controller.context.updater.subscribe())
    }

    pub fn bootstrap(&self) -> Result<()> {
        self.with_lock(|controller| controller.bootstrap())
    }

    pub fn check_for_updates(&self) {
        self.with_lock(|controller| controller.context.updater.check_now());
    }

    pub fn install_update(&self) {
        self.with_lock(|controller| controller.context.updater.install_update());
    }

    pub fn dismiss_update(&self) {
        self.with_lock(|controller| controller.context.updater.dismiss_update());
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
        self.with_lock(|controller| controller.aggregate_vault_status())
    }

    pub fn try_unlock_with_device(&self) -> Result<bool> {
        self.with_lock(|controller| {
            let vault_id = controller
                .default_target_vault_id_for_actions()
                .ok_or_else(|| anyhow!("no vault available to unlock"))?;
            controller.try_unlock_vault_with_device(&vault_id)
        })
    }

    pub fn create_vault(&self, passphrase: &SecretString, device_name: &str) -> Result<()> {
        self.with_lock(|controller| {
            controller.create_named_vault("Personal".into(), passphrase, device_name)?;
            Ok(())
        })
    }

    pub fn unlock_vault(&self, passphrase: &SecretString, device_name: &str) -> Result<()> {
        self.with_lock(|controller| {
            let vault_id = controller
                .default_target_vault_id_for_actions()
                .ok_or_else(|| anyhow!("no vault available to unlock"))?;
            controller.unlock_vault(&vault_id, passphrase, device_name)
        })
    }

    pub fn lock_vault(&self) {
        self.with_lock(|controller| {
            if let Some(vault_id) = controller.default_target_vault_id_for_actions() {
                let _ = controller.lock_vault(&vault_id);
            }
        });
    }

    pub fn list_vaults(&self) -> Vec<ManagedVaultSummary> {
        self.with_lock(|controller| controller.list_vaults())
    }

    pub fn create_named_vault(
        &self,
        name: String,
        passphrase: &SecretString,
        device_name: &str,
    ) -> Result<ManagedVaultSummary> {
        self.with_lock(|controller| controller.create_named_vault(name, passphrase, device_name))
    }

    pub fn rename_vault(&self, vault_id: &str, name: String) -> Result<ManagedVaultSummary> {
        self.with_lock(|controller| controller.rename_vault(vault_id, name))
    }

    pub fn open_vault(&self, vault_id: &str) -> Result<()> {
        self.with_lock(|controller| controller.open_vault(vault_id))
    }

    pub fn close_vault(&self, vault_id: &str) -> Result<()> {
        self.with_lock(|controller| controller.close_vault(vault_id))
    }

    pub fn unlock_named_vault(
        &self,
        vault_id: &str,
        passphrase: &SecretString,
        device_name: &str,
    ) -> Result<()> {
        self.with_lock(|controller| controller.unlock_vault(vault_id, passphrase, device_name))
    }

    pub fn try_unlock_vault_with_device(&self, vault_id: &str) -> Result<bool> {
        self.with_lock(|controller| controller.try_unlock_vault_with_device(vault_id))
    }

    pub fn lock_named_vault(&self, vault_id: &str) -> Result<()> {
        self.with_lock(|controller| controller.lock_vault(vault_id))
    }

    pub fn delete_vault_permanently(&self, vault_id: &str) -> Result<()> {
        self.with_lock(|controller| controller.delete_vault_permanently(vault_id))
    }

    pub fn set_default_target_vault(&self, vault_id: &str) -> Result<()> {
        self.with_lock(|controller| controller.set_default_target_vault(vault_id))
    }

    pub fn list_hosts(&self) -> Result<Vec<VaultScopedHostSummary>> {
        self.with_lock(|controller| controller.list_hosts())
    }

    pub fn load_host(&self, vault_id: &str, id: &str) -> Result<Option<VaultHostProfile>> {
        self.with_lock(|controller| controller.load_host(vault_id, id))
    }

    pub fn save_host(
        &self,
        vault_id: &str,
        host: VaultHostProfile,
    ) -> Result<VaultScopedHostSummary> {
        self.with_lock(|controller| controller.save_host(vault_id, host))
    }

    pub fn delete_host(&self, vault_id: &str, id: &str) -> Result<bool> {
        self.with_lock(|controller| controller.delete_host(vault_id, id))
    }

    pub fn list_password_credentials(&self) -> Result<Vec<VaultScopedCredentialSummary>> {
        self.with_lock(|controller| controller.list_password_credentials())
    }

    pub fn load_password_credential(
        &self,
        vault_id: &str,
        id: &str,
    ) -> Result<Option<VaultPasswordCredential>> {
        self.with_lock(|controller| controller.load_password_credential(vault_id, id))
    }

    pub fn save_password_credential(
        &self,
        vault_id: &str,
        credential: VaultPasswordCredential,
    ) -> Result<VaultScopedCredentialSummary> {
        self.with_lock(|controller| controller.save_password_credential(vault_id, credential))
    }

    pub fn delete_password_credential(&self, vault_id: &str, id: &str) -> Result<bool> {
        self.with_lock(|controller| controller.delete_password_credential(vault_id, id))
    }

    pub fn list_private_keys(&self) -> Result<Vec<VaultScopedKeySummary>> {
        self.with_lock(|controller| controller.list_private_keys())
    }

    pub fn generate_private_key(
        &self,
        vault_id: &str,
        request: GenerateKeyRequest,
    ) -> Result<VaultScopedKeySummary> {
        self.with_lock(|controller| controller.generate_private_key(vault_id, request))
    }

    pub fn delete_private_key(&self, vault_id: &str, id: &str) -> Result<bool> {
        self.with_lock(|controller| controller.delete_private_key(vault_id, id))
    }

    pub fn build_connect_request(&self, vault_id: &str, host_id: &str) -> Result<SshConnectRequest> {
        self.with_lock(|controller| {
            let vault = controller.store(vault_id)?;
            let host = vault
                .load_host_profile(host_id)?
                .ok_or_else(|| anyhow!("saved host not found"))?;

            let mut auth_order = Vec::with_capacity(host.auth_order.len());
            for auth in &host.auth_order {
                match auth {
                    HostAuthRef::Password { credential_id } => {
                        let credential = vault
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
                        let key = vault
                            .load_private_key(key_id)?
                            .ok_or_else(|| anyhow!("missing private key"))?;
                        let passphrase = passphrase_credential_id
                            .as_ref()
                            .map(|id| vault.load_password_credential(id))
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
        for state in self.context.vaults.values_mut() {
            if let Some(store) = state.store.as_mut()
                && store.status().initialized
            {
                let _ = store.try_unlock_with_device();
                self.device_unlock_attempted = true;
            }
        }
        if self.sessions.is_empty() {
            let _ = self.spawn_local_session()?;
        }
        self.context.updater.startup_check();
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
        Ok(WindowBootstrap {
            attached_session_id,
            managed_vaults: self.list_vaults(),
            saved_hosts: self.list_hosts().unwrap_or_default(),
            cached_credentials: self.list_password_credentials().unwrap_or_default(),
            cached_keys: self.list_private_keys().unwrap_or_default(),
            device_unlock_attempted: self.device_unlock_attempted,
            config: self.context.config.snapshot(),
            update_state: self.context.updater.state_snapshot(),
        })
    }

    fn spawn_local_session(&mut self) -> Result<Arc<dyn TerminalSession>> {
        let shell_override = self.context.config.snapshot().terminal.local_shell;
        let session: Arc<dyn TerminalSession> =
            Arc::new(self.context.local.spawn(shell_override.as_deref())?);
        self.register_session(Arc::clone(&session), SessionKind::Local);
        Ok(session)
    }

    fn update_config(&mut self, f: impl FnOnce(&mut AppConfig)) -> Result<AppConfig> {
        let snapshot = self
            .context
            .config
            .update(f)
            .context("failed to persist app config")?;
        self.lifecycle_policy = LifecyclePolicy::from(&snapshot);
        self.context
            .updater
            .update_settings(update_settings_from_config(&snapshot));
        self.publish_config_update(snapshot.clone());
        Ok(snapshot)
    }

    fn reset_config_to_defaults(&mut self) -> Result<AppConfig> {
        let defaults = AppConfig::default();
        self.context
            .config
            .replace(defaults)
            .context("failed to reset app config to defaults")?;
        let snapshot = self.context.config.snapshot();
        self.lifecycle_policy = LifecyclePolicy::from(&snapshot);
        self.context
            .updater
            .update_settings(update_settings_from_config(&snapshot));
        self.publish_config_update(snapshot.clone());
        Ok(snapshot)
    }

    fn subscribe_config_changes(&mut self) -> Receiver<AppConfig> {
        let (tx, rx) = mpsc::channel();
        self.config_subscribers.push(tx);
        rx
    }

    fn publish_config_update(&mut self, snapshot: AppConfig) {
        self.config_subscribers
            .retain(|subscriber| subscriber.send(snapshot.clone()).is_ok());
    }

    fn register_session(&mut self, session: Arc<dyn TerminalSession>, kind: SessionKind) {
        self.bump_access_seq();
        let seq = self.access_seq;
        self.sessions.insert(session, kind, seq);
    }

    fn bump_access_seq(&mut self) {
        self.access_seq = self.access_seq.wrapping_add(1);
    }

    fn aggregate_vault_status(&self) -> VaultStatus {
        let mut open_vaults = self.context.vaults.values().filter_map(|state| state.store.as_ref());
        if let Some(store) = open_vaults.next() {
            return store.status();
        }

        VaultStatus {
            initialized: self
                .context
                .vaults
                .values()
                .any(|state| state.entry.db_file == LEGACY_VAULT_DB_FILE || state.store.is_some()),
            unlocked: false,
            vault_path: self.context.paths.vault_db_path.display().to_string(),
            device_unlock_available: false,
            device_unlock_message: None,
        }
    }

    fn list_vaults(&self) -> Vec<ManagedVaultSummary> {
        let mut vaults = self
            .context
            .vaults
            .values()
            .map(|state| state.summary(&self.context.paths))
            .collect::<Vec<_>>();
        vaults.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
        vaults
    }

    fn create_named_vault(
        &mut self,
        name: String,
        passphrase: &SecretString,
        device_name: &str,
    ) -> Result<ManagedVaultSummary> {
        let trimmed_name = name.trim();
        if trimmed_name.is_empty() {
            anyhow::bail!("vault name cannot be empty");
        }
        if self
            .context
            .vaults
            .values()
            .any(|state| state.entry.name.eq_ignore_ascii_case(trimmed_name))
        {
            anyhow::bail!("a vault named '{}' already exists", trimmed_name);
        }

        fs::create_dir_all(&self.context.paths.vaults_dir)
            .context("failed to create vault storage directory")?;
        let vault_id = Uuid::new_v4().to_string();
        let db_file = format!("{vault_id}.sqlite");
        let db_path = self.context.paths.vaults_dir.join(&db_file);
        let mut store = VaultStore::open(&db_path).context("failed to create vault database")?;
        store
            .create_vault(passphrase, device_name)
            .context("failed to initialize vault")?;

        let now = seance_vault::now_ts();
        let entry = VaultRegistryEntry {
            id: vault_id.clone(),
            name: trimmed_name.to_string(),
            db_file,
            created_at: now,
            updated_at: now,
        };
        if let Err(error) = self.update_config(|config| {
            config.vaults.entries.push(entry.clone());
            config.vaults.open_vault_ids.push(vault_id.clone());
            if config.vaults.default_target_vault_id.is_none() {
                config.vaults.default_target_vault_id = Some(vault_id.clone());
            }
        }) {
            let _ = fs::remove_file(&db_path);
            return Err(error);
        }

        self.context.vaults.insert(
            vault_id.clone(),
            ManagedVaultState {
                entry,
                store: Some(store),
                availability_error: None,
            },
        );
        Ok(self
            .context
            .vaults
            .get(&vault_id)
            .expect("inserted vault state")
            .summary(&self.context.paths))
    }

    fn rename_vault(&mut self, vault_id: &str, name: String) -> Result<ManagedVaultSummary> {
        let trimmed_name = name.trim();
        if trimmed_name.is_empty() {
            anyhow::bail!("vault name cannot be empty");
        }
        if self.context.vaults.iter().any(|(id, state)| {
            id != vault_id && state.entry.name.eq_ignore_ascii_case(trimmed_name)
        }) {
            anyhow::bail!("a vault named '{}' already exists", trimmed_name);
        }

        let state = self
            .context
            .vaults
            .get_mut(vault_id)
                .ok_or_else(|| anyhow!("vault not found"))?;
        state.entry.name = trimmed_name.to_string();
        state.entry.updated_at = seance_vault::now_ts();
        let updated_entry = state.entry.clone();
        let summary = state.summary(&self.context.paths);
        let _ = state;
        self.update_config(|config| {
            if let Some(entry) = config.vaults.entries.iter_mut().find(|entry| entry.id == vault_id) {
                *entry = updated_entry.clone();
            }
        })?;
        Ok(summary)
    }

    fn open_vault(&mut self, vault_id: &str) -> Result<()> {
        let state = self
            .context
            .vaults
            .get_mut(vault_id)
            .ok_or_else(|| anyhow!("vault not found"))?;
        if state.store.is_none() {
            let path = vault_db_path(&self.context.paths, &state.entry);
            let store = VaultStore::open(&path).with_context(|| {
                format!("failed to open vault database at {}", path.display())
            })?;
            state.store = Some(store);
            state.availability_error = None;
        }
        if !self
            .context
            .config
            .snapshot()
            .vaults
            .open_vault_ids
            .iter()
            .any(|id| id == vault_id)
        {
            self.update_config(|config| {
                config.vaults.open_vault_ids.push(vault_id.to_string());
            })?;
        }
        Ok(())
    }

    fn close_vault(&mut self, vault_id: &str) -> Result<()> {
        let state = self
            .context
            .vaults
            .get_mut(vault_id)
            .ok_or_else(|| anyhow!("vault not found"))?;
        state.store = None;
        self.update_config(|config| {
            config.vaults.open_vault_ids.retain(|id| id != vault_id);
        })?;
        Ok(())
    }

    fn unlock_vault(
        &mut self,
        vault_id: &str,
        passphrase: &SecretString,
        device_name: &str,
    ) -> Result<()> {
        self.open_vault(vault_id)?;
        let store = self.store_mut(vault_id)?;
        store
            .unlock_with_passphrase(passphrase, device_name)
            .context("failed to unlock vault")?;
        self.device_unlock_attempted = true;
        Ok(())
    }

    fn try_unlock_vault_with_device(&mut self, vault_id: &str) -> Result<bool> {
        self.open_vault(vault_id)?;
        self.device_unlock_attempted = true;
        let store = self.store_mut(vault_id)?;
        Ok(store.try_unlock_with_device()?)
    }

    fn lock_vault(&mut self, vault_id: &str) -> Result<()> {
        let store = self.store_mut(vault_id)?;
        store.lock();
        Ok(())
    }

    fn delete_vault_permanently(&mut self, vault_id: &str) -> Result<()> {
        let state = self
            .context
            .vaults
            .get(vault_id)
            .ok_or_else(|| anyhow!("vault not found"))?;
        if state.store.as_ref().is_some_and(|store| store.status().unlocked) {
            anyhow::bail!("lock the vault before deleting it");
        }
        let path = vault_db_path(&self.context.paths, &state.entry);
        self.context.vaults.remove(vault_id);
        self.update_config(|config| {
            config.vaults.entries.retain(|entry| entry.id != vault_id);
            config.vaults.open_vault_ids.retain(|id| id != vault_id);
            if config.vaults.default_target_vault_id.as_deref() == Some(vault_id) {
                config.vaults.default_target_vault_id = None;
            }
        })?;
        if path.exists() {
            fs::remove_file(&path)
                .with_context(|| format!("failed to remove vault at {}", path.display()))?;
        }
        Ok(())
    }

    fn set_default_target_vault(&mut self, vault_id: &str) -> Result<()> {
        if !self.context.vaults.contains_key(vault_id) {
            anyhow::bail!("vault not found");
        }
        self.update_config(|config| {
            config.vaults.default_target_vault_id = Some(vault_id.to_string());
        })?;
        Ok(())
    }

    fn list_hosts(&self) -> Result<Vec<VaultScopedHostSummary>> {
        let mut hosts = Vec::new();
        for (vault_id, state) in &self.context.vaults {
            let Some(store) = state.store.as_ref() else {
                continue;
            };
            if !store.status().unlocked {
                continue;
            }
            hosts.extend(
                store
                    .list_host_profiles()?
                    .into_iter()
                    .map(|host| VaultScopedHostSummary {
                        vault_id: vault_id.clone(),
                        vault_name: state.entry.name.clone(),
                        host,
                    }),
            );
        }
        hosts.sort_by(|left, right| {
            left.host
                .label
                .to_lowercase()
                .cmp(&right.host.label.to_lowercase())
                .then_with(|| left.vault_name.to_lowercase().cmp(&right.vault_name.to_lowercase()))
        });
        Ok(hosts)
    }

    fn load_host(&self, vault_id: &str, id: &str) -> Result<Option<VaultHostProfile>> {
        Ok(self.store(vault_id)?.load_host_profile(id)?)
    }

    fn save_host(&mut self, vault_id: &str, host: VaultHostProfile) -> Result<VaultScopedHostSummary> {
        let summary = self.store_mut(vault_id)?.store_host_profile(host)?;
        let vault_name = self.vault_entry(vault_id)?.name.clone();
        Ok(VaultScopedHostSummary {
            vault_id: vault_id.to_string(),
            vault_name,
            host: summary,
        })
    }

    fn delete_host(&mut self, vault_id: &str, id: &str) -> Result<bool> {
        Ok(self.store_mut(vault_id)?.delete_host_profile(id)?)
    }

    fn list_password_credentials(&self) -> Result<Vec<VaultScopedCredentialSummary>> {
        let mut credentials = Vec::new();
        for (vault_id, state) in &self.context.vaults {
            let Some(store) = state.store.as_ref() else {
                continue;
            };
            if !store.status().unlocked {
                continue;
            }
            credentials.extend(
                store
                    .list_password_credentials()?
                    .into_iter()
                    .map(|credential| VaultScopedCredentialSummary {
                        vault_id: vault_id.clone(),
                        vault_name: state.entry.name.clone(),
                        credential,
                    }),
            );
        }
        credentials.sort_by(|left, right| {
            left.credential
                .label
                .to_lowercase()
                .cmp(&right.credential.label.to_lowercase())
                .then_with(|| left.vault_name.to_lowercase().cmp(&right.vault_name.to_lowercase()))
        });
        Ok(credentials)
    }

    fn load_password_credential(
        &self,
        vault_id: &str,
        id: &str,
    ) -> Result<Option<VaultPasswordCredential>> {
        Ok(self.store(vault_id)?.load_password_credential(id)?)
    }

    fn save_password_credential(
        &mut self,
        vault_id: &str,
        credential: VaultPasswordCredential,
    ) -> Result<VaultScopedCredentialSummary> {
        let summary = self.store_mut(vault_id)?.store_password_credential(credential)?;
        let vault_name = self.vault_entry(vault_id)?.name.clone();
        Ok(VaultScopedCredentialSummary {
            vault_id: vault_id.to_string(),
            vault_name,
            credential: summary,
        })
    }

    fn delete_password_credential(&mut self, vault_id: &str, id: &str) -> Result<bool> {
        Ok(self.store_mut(vault_id)?.delete_password_credential(id)?)
    }

    fn list_private_keys(&self) -> Result<Vec<VaultScopedKeySummary>> {
        let mut keys = Vec::new();
        for (vault_id, state) in &self.context.vaults {
            let Some(store) = state.store.as_ref() else {
                continue;
            };
            if !store.status().unlocked {
                continue;
            }
            keys.extend(
                store
                    .list_private_keys()?
                    .into_iter()
                    .map(|key| VaultScopedKeySummary {
                        vault_id: vault_id.clone(),
                        vault_name: state.entry.name.clone(),
                        key,
                    }),
            );
        }
        keys.sort_by(|left, right| {
            left.key
                .label
                .to_lowercase()
                .cmp(&right.key.label.to_lowercase())
                .then_with(|| left.vault_name.to_lowercase().cmp(&right.vault_name.to_lowercase()))
        });
        Ok(keys)
    }

    fn generate_private_key(
        &mut self,
        vault_id: &str,
        request: GenerateKeyRequest,
    ) -> Result<VaultScopedKeySummary> {
        let summary = self.store_mut(vault_id)?.generate_private_key(request)?;
        let vault_name = self.vault_entry(vault_id)?.name.clone();
        Ok(VaultScopedKeySummary {
            vault_id: vault_id.to_string(),
            vault_name,
            key: summary,
        })
    }

    fn delete_private_key(&mut self, vault_id: &str, id: &str) -> Result<bool> {
        Ok(self.store_mut(vault_id)?.delete_private_key(id)?)
    }

    fn default_target_vault_id_for_actions(&self) -> Option<String> {
        let config = self.context.config.snapshot();
        if let Some(default_id) = config.vaults.default_target_vault_id
            && self
                .context
                .vaults
                .get(&default_id)
                .is_some_and(|state| state.store.as_ref().is_some_and(|store| store.status().unlocked))
        {
            return Some(default_id);
        }

        let mut unlocked = self
            .context
            .vaults
            .iter()
            .filter(|(_, state)| state.store.as_ref().is_some_and(|store| store.status().unlocked))
            .map(|(vault_id, state)| (vault_id.clone(), state.entry.name.to_lowercase()))
            .collect::<Vec<_>>();
        unlocked.sort_by(|left, right| left.1.cmp(&right.1));
        unlocked.into_iter().map(|(vault_id, _)| vault_id).next()
    }

    fn vault_entry(&self, vault_id: &str) -> Result<&VaultRegistryEntry> {
        Ok(&self
            .context
            .vaults
            .get(vault_id)
            .ok_or_else(|| anyhow!("vault not found"))?
            .entry)
    }

    fn store(&self, vault_id: &str) -> Result<&VaultStore> {
        self.context
            .vaults
            .get(vault_id)
            .and_then(|state| state.store.as_ref())
            .ok_or_else(|| anyhow!("vault is not open"))
    }

    fn store_mut(&mut self, vault_id: &str) -> Result<&mut VaultStore> {
        self.context
            .vaults
            .get_mut(vault_id)
            .and_then(|state| state.store.as_mut())
            .ok_or_else(|| anyhow!("vault is not open"))
    }
}

impl ManagedVaultState {
    fn summary(&self, paths: &AppPaths) -> ManagedVaultSummary {
        let db_path = vault_db_path(paths, &self.entry);
        let status = self.store.as_ref().map(VaultStore::status);
        let (host_count, credential_count, key_count) = if let Some(store) = self.store.as_ref() {
            if store.status().unlocked {
                (
                    store.list_host_profiles().unwrap_or_default().len(),
                    store.list_password_credentials().unwrap_or_default().len(),
                    store.list_private_keys().unwrap_or_default().len(),
                )
            } else {
                (0, 0, 0)
            }
        } else {
            (0, 0, 0)
        };

        ManagedVaultSummary {
            vault_id: self.entry.id.clone(),
            name: self.entry.name.clone(),
            db_path,
            open: self.store.is_some(),
            initialized: status.as_ref().is_some_and(|status| status.initialized),
            unlocked: status.as_ref().is_some_and(|status| status.unlocked),
            device_unlock_available: status
                .as_ref()
                .is_some_and(|status| status.device_unlock_available),
            device_unlock_message: status
                .as_ref()
                .and_then(|status| status.device_unlock_message.clone()),
            host_count,
            credential_count,
            key_count,
            availability_error: self.availability_error.clone(),
        }
    }
}

fn migrate_legacy_vault_registry(paths: &AppPaths, config: &mut ConfigStore) -> Result<()> {
    let snapshot = config.snapshot();
    if !snapshot.vaults.entries.is_empty() || !paths.vault_db_path.exists() {
        return Ok(());
    }

    let now = seance_vault::now_ts();
    let legacy_entry = VaultRegistryEntry {
        id: Uuid::new_v4().to_string(),
        name: "Personal".into(),
        db_file: LEGACY_VAULT_DB_FILE.into(),
        created_at: now,
        updated_at: now,
    };
    config
        .update(|config| {
            config.vaults.entries.push(legacy_entry.clone());
            config.vaults.open_vault_ids.push(legacy_entry.id.clone());
            config.vaults.default_target_vault_id = Some(legacy_entry.id.clone());
        })
        .context("failed to migrate legacy vault registry")?;
    Ok(())
}

fn load_managed_vaults(
    paths: &AppPaths,
    config: &AppConfig,
) -> Result<HashMap<String, ManagedVaultState>> {
    let mut vaults = HashMap::new();
    for entry in &config.vaults.entries {
        let mut state = ManagedVaultState {
            entry: entry.clone(),
            store: None,
            availability_error: None,
        };
        if config.vaults.open_vault_ids.iter().any(|id| id == &entry.id) {
            let db_path = vault_db_path(paths, entry);
            match VaultStore::open(&db_path) {
                Ok(store) => state.store = Some(store),
                Err(error) => state.availability_error = Some(error.to_string()),
            }
        }
        vaults.insert(entry.id.clone(), state);
    }
    Ok(vaults)
}

fn vault_db_path(paths: &AppPaths, entry: &VaultRegistryEntry) -> PathBuf {
    if entry.db_file == LEGACY_VAULT_DB_FILE && paths.vault_db_path.exists() {
        paths.vault_db_path.clone()
    } else {
        paths.vaults_dir.join(&entry.db_file)
    }
}

fn update_settings_from_config(config: &AppConfig) -> UpdateSettings {
    UpdateSettings {
        auto_check: config.updates.auto_check,
        install_mode: match config.updates.install_mode {
            seance_config::UpdateInstallMode::Prompted => InstallMode::Prompted,
        },
        channel: match config.updates.channel {
            seance_config::UpdateReleaseChannel::Stable => ReleaseChannel::Stable,
        },
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{Arc, mpsc},
        time::Duration,
    };

    use anyhow::Result;
    use seance_terminal::{
        SessionPerfSnapshot, SessionSnapshot, TerminalGeometry, TerminalSession,
    };
    use tempfile::tempdir;

    use super::{
        AppContext, AppControllerHandle, AppPaths, LifecyclePolicy, PlatformCloseAction,
        SessionKind, SessionRegistry,
    };

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

    #[test]
    fn detected_paths_include_config_toml() {
        let paths = AppPaths::detect().unwrap();
        assert_eq!(paths.config_path.file_name().unwrap(), "config.toml");
        assert_eq!(paths.vaults_dir.file_name().unwrap(), "vaults");
    }

    #[test]
    fn updating_config_changes_lifecycle_behavior_immediately() {
        let controller = make_test_controller();
        assert_eq!(
            controller.on_last_window_closed(),
            PlatformCloseAction::Hide
        );

        let config = controller
            .update_config(|config| {
                config.window.keep_running_without_windows = false;
            })
            .unwrap();

        assert!(!config.window.keep_running_without_windows);
        assert_eq!(
            controller.on_last_window_closed(),
            PlatformCloseAction::Exit
        );
    }

    #[test]
    fn reset_config_restores_defaults() {
        let controller = make_test_controller();
        controller
            .update_config(|config| {
                config.appearance.theme = "bone".into();
                config.window.keep_running_without_windows = false;
            })
            .unwrap();

        let reset = controller.reset_config_to_defaults().unwrap();

        assert_eq!(reset.appearance.theme, "obsidian-smoke");
        assert!(reset.window.keep_running_without_windows);
        assert_eq!(
            controller.on_last_window_closed(),
            PlatformCloseAction::Hide
        );
    }

    #[test]
    fn update_config_broadcasts_new_snapshot() {
        let controller = make_test_controller();
        let rx = controller.subscribe_config_changes();

        let updated = controller
            .update_config(|config| {
                config.appearance.theme = "bone".into();
            })
            .unwrap();

        let broadcast = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(broadcast, updated);
        assert_eq!(broadcast.appearance.theme, "bone");
    }

    #[test]
    fn reset_config_broadcasts_defaults() {
        let controller = make_test_controller();
        controller
            .update_config(|config| {
                config.appearance.theme = "bone".into();
            })
            .unwrap();
        let rx = controller.subscribe_config_changes();

        let reset = controller.reset_config_to_defaults().unwrap();

        let broadcast = rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(broadcast, reset);
        assert_eq!(broadcast.appearance.theme, "obsidian-smoke");
    }

    #[test]
    fn dead_config_subscribers_do_not_break_broadcasts() {
        let controller = make_test_controller();
        let rx = controller.subscribe_config_changes();
        drop(rx);

        let active_rx = controller.subscribe_config_changes();
        controller
            .update_config(|config| {
                config.appearance.theme = "nord".into();
            })
            .unwrap();

        let broadcast = active_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(broadcast.appearance.theme, "nord");
    }

    fn make_test_controller() -> AppControllerHandle {
        let dir = tempdir().unwrap();
        let root = dir.keep();
        let context = AppContext::open(AppPaths {
            app_root: root.clone(),
            config_path: root.join("config.toml"),
            vault_db_path: root.join("vault.sqlite"),
            vaults_dir: root.join("vaults"),
            ipc_socket_path: root.join("resident.sock"),
            instance_lock_path: root.join("resident.lock"),
        })
        .unwrap();
        AppControllerHandle::new(context)
    }
}
