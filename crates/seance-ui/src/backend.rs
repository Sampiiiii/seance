use std::sync::{Arc, mpsc::Receiver};

use anyhow::Result;
use seance_config::{AppConfig, PerfHudDefault, TerminalConfig, WindowConfig};
use seance_core::{
    AppControllerHandle, ManagedVaultSummary, SessionId, SessionKind, UpdateState,
    VaultScopedCredentialSummary, VaultScopedHostSummary, VaultScopedKeySummary,
};
use seance_ssh::{SftpEntry, SshConnectRequest, SshSessionManager};
use seance_terminal::TerminalSession;
use seance_vault::{
    GenerateKeyAlgorithm, GenerateKeyRequest, ImportKeyRequest, KeySummary, SecretString,
    VaultHostProfile, VaultPasswordCredential, VaultStatus,
};

#[derive(Clone)]
pub struct UiBackend {
    controller: AppControllerHandle,
}

impl UiBackend {
    pub fn new(controller: AppControllerHandle) -> Result<Self> {
        Ok(Self { controller })
    }

    pub fn controller(&self) -> &AppControllerHandle {
        &self.controller
    }

    pub fn subscribe_config_changes(&self) -> Receiver<AppConfig> {
        self.controller.subscribe_config_changes()
    }

    pub fn subscribe_update_changes(&self) -> Receiver<UpdateState> {
        self.controller.subscribe_update_changes()
    }

    pub fn set_theme(&self, theme: String) -> Result<AppConfig> {
        self.controller.update_config(|config| {
            config.appearance.theme = theme;
        })
    }

    pub fn set_window_settings(&self, window: WindowConfig) -> Result<AppConfig> {
        self.controller.update_config(|config| {
            config.window = window;
        })
    }

    pub fn set_terminal_settings(&self, terminal: TerminalConfig) -> Result<AppConfig> {
        self.controller.update_config(|config| {
            config.terminal = terminal;
        })
    }

    pub fn set_perf_hud_default(&self, perf_hud_default: PerfHudDefault) -> Result<AppConfig> {
        self.controller.update_config(|config| {
            config.debug.perf_hud_default = perf_hud_default;
        })
    }

    pub fn reset_settings_to_defaults(&self) -> Result<AppConfig> {
        self.controller.reset_config_to_defaults()
    }

    pub fn check_for_updates(&self) {
        self.controller.check_for_updates();
    }

    pub fn install_update(&self) {
        self.controller.install_update();
    }

    pub fn dismiss_update(&self) {
        self.controller.dismiss_update();
    }

    pub fn ssh_manager(&self) -> Arc<SshSessionManager> {
        self.controller.ssh_manager()
    }

    pub fn vault_status(&self) -> VaultStatus {
        self.controller.vault_status()
    }

    pub fn list_vaults(&self) -> Vec<ManagedVaultSummary> {
        self.controller.list_vaults()
    }

    #[allow(dead_code)]
    pub fn try_unlock_with_device(&self, vault_id: &str) -> Result<bool> {
        self.controller.try_unlock_vault_with_device(vault_id)
    }

    pub fn create_vault(
        &self,
        name: String,
        passphrase: &SecretString,
        device_name: &str,
    ) -> Result<ManagedVaultSummary> {
        self.controller
            .create_named_vault(name, passphrase, device_name)
    }

    pub fn rename_vault(&self, vault_id: &str, name: String) -> Result<ManagedVaultSummary> {
        self.controller.rename_vault(vault_id, name)
    }

    pub fn open_vault(&self, vault_id: &str) -> Result<()> {
        self.controller.open_vault(vault_id)
    }

    pub fn close_vault(&self, vault_id: &str) -> Result<()> {
        self.controller.close_vault(vault_id)
    }

    pub fn unlock_vault(
        &self,
        vault_id: &str,
        passphrase: &SecretString,
        device_name: &str,
    ) -> Result<()> {
        self.controller
            .unlock_named_vault(vault_id, passphrase, device_name)
    }

    pub fn lock_vault(&self, vault_id: &str) -> Result<()> {
        self.controller.lock_named_vault(vault_id)
    }

    pub fn delete_vault_permanently(&self, vault_id: &str) -> Result<()> {
        self.controller.delete_vault_permanently(vault_id)
    }

    pub fn set_default_target_vault(&self, vault_id: &str) -> Result<()> {
        self.controller.set_default_target_vault(vault_id)
    }

    pub fn spawn_local_session(&self) -> Result<Arc<dyn TerminalSession>> {
        self.controller.spawn_local_session()
    }

    pub fn register_remote_session(&self, session: Arc<dyn TerminalSession>) {
        self.controller.register_remote_session(session);
    }

    pub fn list_sessions(&self) -> Vec<Arc<dyn TerminalSession>> {
        self.controller.list_sessions()
    }

    pub fn recent_session_id(&self) -> Option<SessionId> {
        self.controller.most_recent_session_id()
    }

    pub fn session_kind(&self, id: SessionId) -> Option<SessionKind> {
        self.controller.session_kind(id)
    }

    pub fn session(&self, id: SessionId) -> Option<Arc<dyn TerminalSession>> {
        self.controller.get_session(id)
    }

    pub fn touch_session(&self, id: SessionId) {
        self.controller.touch_session(id);
    }

    pub fn close_session(&self, id: SessionId) -> bool {
        self.controller.close_session(id)
    }

    pub fn list_hosts(&self) -> Result<Vec<VaultScopedHostSummary>> {
        self.controller.list_hosts()
    }

    pub fn load_host(&self, vault_id: &str, id: &str) -> Result<Option<VaultHostProfile>> {
        self.controller.load_host(vault_id, id)
    }

    pub fn save_host(
        &self,
        vault_id: &str,
        host: VaultHostProfile,
    ) -> Result<VaultScopedHostSummary> {
        self.controller.save_host(vault_id, host)
    }

    pub fn delete_host(&self, vault_id: &str, id: &str) -> Result<bool> {
        self.controller.delete_host(vault_id, id)
    }

    pub fn list_password_credentials(&self) -> Result<Vec<VaultScopedCredentialSummary>> {
        self.controller.list_password_credentials()
    }

    pub fn load_password_credential(
        &self,
        vault_id: &str,
        id: &str,
    ) -> Result<Option<VaultPasswordCredential>> {
        self.controller.load_password_credential(vault_id, id)
    }

    pub fn save_password_credential(
        &self,
        vault_id: &str,
        credential: VaultPasswordCredential,
    ) -> Result<VaultScopedCredentialSummary> {
        self.controller
            .save_password_credential(vault_id, credential)
    }

    pub fn delete_password_credential(&self, vault_id: &str, id: &str) -> Result<bool> {
        self.controller.delete_password_credential(vault_id, id)
    }

    pub fn list_private_keys(&self) -> Result<Vec<VaultScopedKeySummary>> {
        self.controller.list_private_keys()
    }

    #[allow(dead_code)]
    pub fn import_private_key(&self, _request: ImportKeyRequest) -> Result<KeySummary> {
        anyhow::bail!("private key import is not yet wired through the resident controller")
    }

    pub fn delete_private_key(&self, vault_id: &str, id: &str) -> Result<bool> {
        self.controller.delete_private_key(vault_id, id)
    }

    pub fn generate_private_key(
        &self,
        vault_id: &str,
        request: GenerateKeyRequest,
    ) -> Result<VaultScopedKeySummary> {
        self.controller.generate_private_key(vault_id, request)
    }

    pub fn generate_ed25519_key(
        &self,
        vault_id: &str,
        label: String,
    ) -> Result<VaultScopedKeySummary> {
        self.generate_private_key(
            vault_id,
            GenerateKeyRequest {
                label,
                algorithm: GenerateKeyAlgorithm::Ed25519,
            },
        )
    }

    pub fn generate_rsa_key(&self, vault_id: &str, label: String) -> Result<VaultScopedKeySummary> {
        self.generate_private_key(
            vault_id,
            GenerateKeyRequest {
                label,
                algorithm: GenerateKeyAlgorithm::Rsa { bits: 4096 },
            },
        )
    }

    pub fn build_connect_request(
        &self,
        vault_id: &str,
        host_id: &str,
    ) -> Result<SshConnectRequest> {
        self.controller.build_connect_request(vault_id, host_id)
    }

    pub fn sftp_canonicalize(&self, session_id: u64, path: &str) -> Result<String> {
        self.controller.sftp_canonicalize(session_id, path)
    }

    pub fn sftp_list_dir(&self, session_id: u64, path: &str) -> Result<Vec<SftpEntry>> {
        self.controller.sftp_list_dir(session_id, path)
    }

    pub fn sftp_read_file(&self, session_id: u64, remote_path: &str) -> Result<Vec<u8>> {
        self.controller.sftp_read_file(session_id, remote_path)
    }

    #[allow(dead_code)]
    pub fn sftp_write_file(&self, session_id: u64, remote_path: &str, data: &[u8]) -> Result<()> {
        self.controller
            .sftp_write_file(session_id, remote_path, data)
    }

    pub fn sftp_mkdir(&self, session_id: u64, path: &str) -> Result<()> {
        self.controller.sftp_mkdir(session_id, path)
    }

    pub fn sftp_remove(&self, session_id: u64, path: &str, is_dir: bool) -> Result<()> {
        self.controller.sftp_remove(session_id, path, is_dir)
    }

    pub fn sftp_rename(&self, session_id: u64, old_path: &str, new_path: &str) -> Result<()> {
        self.controller.sftp_rename(session_id, old_path, new_path)
    }
}
