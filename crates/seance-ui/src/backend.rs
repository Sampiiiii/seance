use std::sync::{Arc, mpsc::Receiver};

use anyhow::Result;
use seance_config::{AppConfig, PerfHudDefault, TerminalConfig, WindowConfig};
use seance_core::{
    AppControllerHandle, DiscoveredPrivateKeyCandidate, HostReferenceSummary,
    ImportPrivateKeyFromPathRequest, ManagedVaultSummary, SessionId, SessionKind,
    SessionMetadataSummary, SessionOrigin, UpdateState, VaultScopedCredentialSummary,
    VaultScopedHostSummary, VaultScopedKeySummary, VaultScopedPortForwardSummary, VaultUiSnapshot,
};
use seance_ssh::{PortForwardRuntimeSnapshot, SftpEntry, SshConnectRequest, SshPortForwardRequest};
use seance_terminal::TerminalSession;
use seance_vault::{
    GenerateKeyAlgorithm, GenerateKeyRequest, ImportKeyRequest, SecretString, VaultHostProfile,
    VaultPasswordCredential, VaultPortForwardProfile, VaultStatus,
};

#[derive(Clone)]
pub(crate) struct UiBackend {
    controller: AppControllerHandle,
}

impl UiBackend {
    pub(crate) fn new(controller: AppControllerHandle) -> Result<Self> {
        Ok(Self { controller })
    }

    pub(crate) fn controller(&self) -> &AppControllerHandle {
        &self.controller
    }

    pub(crate) fn subscribe_config_changes(&self) -> Receiver<AppConfig> {
        self.controller.subscribe_config_changes()
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn subscribe_update_changes(&self) -> Receiver<UpdateState> {
        self.controller.subscribe_update_changes()
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn subscribe_vault_changes(&self) -> Receiver<VaultUiSnapshot> {
        self.controller.subscribe_vault_changes()
    }

    #[cfg_attr(test, allow(dead_code))]
    pub(crate) fn subscribe_tunnel_state_changes(
        &self,
    ) -> Receiver<Vec<PortForwardRuntimeSnapshot>> {
        self.controller.subscribe_tunnel_state_changes()
    }

    pub(crate) fn set_theme(&self, theme: String) -> Result<AppConfig> {
        self.controller.update_config(|config| {
            config.appearance.theme = theme;
        })
    }

    pub(crate) fn set_window_settings(&self, window: WindowConfig) -> Result<AppConfig> {
        self.controller.update_config(|config| {
            config.window = window;
        })
    }

    pub(crate) fn set_terminal_settings(&self, terminal: TerminalConfig) -> Result<AppConfig> {
        self.controller.update_config(|config| {
            config.terminal = terminal;
        })
    }

    pub(crate) fn set_perf_hud_default(
        &self,
        perf_hud_default: PerfHudDefault,
    ) -> Result<AppConfig> {
        self.controller.update_config(|config| {
            config.debug.perf_hud_default = perf_hud_default;
        })
    }

    pub(crate) fn reset_settings_to_defaults(&self) -> Result<AppConfig> {
        self.controller.reset_config_to_defaults()
    }

    pub(crate) fn check_for_updates(&self) {
        self.controller.check_for_updates();
    }

    pub(crate) fn install_update(&self) {
        self.controller.install_update();
    }

    pub(crate) fn dismiss_update(&self) {
        self.controller.dismiss_update();
    }

    pub(crate) fn start_connect(
        &self,
        request: SshConnectRequest,
    ) -> std::result::Result<seance_ssh::SshConnectTask, seance_ssh::SshError> {
        self.controller.start_connect(request)
    }

    pub(crate) fn vault_status(&self) -> VaultStatus {
        self.controller.vault_status()
    }

    pub(crate) fn list_vaults(&self) -> Vec<ManagedVaultSummary> {
        self.controller.list_vaults()
    }

    pub(crate) fn try_unlock_with_device(&self, vault_id: &str) -> Result<bool> {
        self.controller.try_unlock_vault_with_device(vault_id)
    }

    pub(crate) fn create_vault(
        &self,
        name: String,
        passphrase: &SecretString,
        device_name: &str,
    ) -> Result<ManagedVaultSummary> {
        self.controller
            .create_named_vault(name, passphrase, device_name)
    }

    pub(crate) fn rename_vault(&self, vault_id: &str, name: String) -> Result<ManagedVaultSummary> {
        self.controller.rename_vault(vault_id, name)
    }

    pub(crate) fn open_vault(&self, vault_id: &str) -> Result<()> {
        self.controller.open_vault(vault_id)
    }

    pub(crate) fn close_vault(&self, vault_id: &str) -> Result<()> {
        self.controller.close_vault(vault_id)
    }

    pub(crate) fn unlock_vault(
        &self,
        vault_id: &str,
        passphrase: &SecretString,
        device_name: &str,
    ) -> Result<()> {
        self.controller
            .unlock_named_vault(vault_id, passphrase, device_name)
    }

    pub(crate) fn lock_vault(&self, vault_id: &str) -> Result<()> {
        self.controller.lock_named_vault(vault_id)
    }

    pub(crate) fn delete_vault_permanently(&self, vault_id: &str) -> Result<()> {
        self.controller.delete_vault_permanently(vault_id)
    }

    pub(crate) fn set_default_target_vault(&self, vault_id: &str) -> Result<()> {
        self.controller.set_default_target_vault(vault_id)
    }

    pub(crate) fn spawn_local_session(&self) -> Result<Arc<dyn TerminalSession>> {
        self.controller.spawn_local_session()
    }

    pub(crate) fn register_remote_session_with_origin(
        &self,
        session: Arc<dyn TerminalSession>,
        origin: SessionOrigin,
    ) {
        self.controller
            .register_remote_session_with_origin(session, origin);
    }

    pub(crate) fn list_sessions(&self) -> Vec<Arc<dyn TerminalSession>> {
        self.controller.list_sessions()
    }

    pub(crate) fn recent_session_id(&self) -> Option<SessionId> {
        self.controller.most_recent_session_id()
    }

    pub(crate) fn session_kind(&self, id: SessionId) -> Option<SessionKind> {
        self.controller.session_kind(id)
    }

    pub(crate) fn session_metadata(&self, id: SessionId) -> Option<SessionMetadataSummary> {
        self.controller.session_metadata(id)
    }

    pub(crate) fn session(&self, id: SessionId) -> Option<Arc<dyn TerminalSession>> {
        self.controller.get_session(id)
    }

    pub(crate) fn touch_session(&self, id: SessionId) {
        self.controller.touch_session(id);
    }

    pub(crate) fn close_session(&self, id: SessionId) -> bool {
        self.controller.close_session(id)
    }

    pub(crate) fn list_hosts(&self) -> Result<Vec<VaultScopedHostSummary>> {
        self.controller.list_hosts()
    }

    pub(crate) fn load_host(&self, vault_id: &str, id: &str) -> Result<Option<VaultHostProfile>> {
        self.controller.load_host(vault_id, id)
    }

    pub(crate) fn save_host(
        &self,
        vault_id: &str,
        host: VaultHostProfile,
    ) -> Result<VaultScopedHostSummary> {
        self.controller.save_host(vault_id, host)
    }

    pub(crate) fn delete_host(&self, vault_id: &str, id: &str) -> Result<bool> {
        self.controller.delete_host(vault_id, id)
    }

    pub(crate) fn list_port_forwards(&self) -> Result<Vec<VaultScopedPortForwardSummary>> {
        self.controller.list_port_forwards()
    }

    pub(crate) fn load_port_forward(
        &self,
        vault_id: &str,
        id: &str,
    ) -> Result<Option<VaultPortForwardProfile>> {
        self.controller.load_port_forward(vault_id, id)
    }

    pub(crate) fn save_port_forward(
        &self,
        vault_id: &str,
        port_forward: VaultPortForwardProfile,
    ) -> Result<VaultScopedPortForwardSummary> {
        self.controller.save_port_forward(vault_id, port_forward)
    }

    pub(crate) fn delete_port_forward(&self, vault_id: &str, id: &str) -> Result<bool> {
        self.controller.delete_port_forward(vault_id, id)
    }

    pub(crate) fn list_password_credentials(&self) -> Result<Vec<VaultScopedCredentialSummary>> {
        self.controller.list_password_credentials()
    }

    pub(crate) fn load_password_credential(
        &self,
        vault_id: &str,
        id: &str,
    ) -> Result<Option<VaultPasswordCredential>> {
        self.controller.load_password_credential(vault_id, id)
    }

    pub(crate) fn save_password_credential(
        &self,
        vault_id: &str,
        credential: VaultPasswordCredential,
    ) -> Result<VaultScopedCredentialSummary> {
        self.controller
            .save_password_credential(vault_id, credential)
    }

    pub(crate) fn delete_password_credential(&self, vault_id: &str, id: &str) -> Result<bool> {
        self.controller.delete_password_credential(vault_id, id)
    }

    pub(crate) fn list_private_keys(&self) -> Result<Vec<VaultScopedKeySummary>> {
        self.controller.list_private_keys()
    }

    pub(crate) fn delete_private_key(&self, vault_id: &str, id: &str) -> Result<bool> {
        self.controller.delete_private_key(vault_id, id)
    }

    pub(crate) fn generate_private_key(
        &self,
        vault_id: &str,
        request: GenerateKeyRequest,
    ) -> Result<VaultScopedKeySummary> {
        self.controller.generate_private_key(vault_id, request)
    }

    pub(crate) fn import_private_key(
        &self,
        vault_id: &str,
        request: ImportKeyRequest,
    ) -> Result<VaultScopedKeySummary> {
        self.controller.import_private_key(vault_id, request)
    }

    pub(crate) fn discover_private_key_candidates(
        &self,
        vault_id: &str,
    ) -> Result<Vec<DiscoveredPrivateKeyCandidate>> {
        self.controller.discover_private_key_candidates(vault_id)
    }

    pub(crate) fn import_private_keys_from_paths(
        &self,
        vault_id: &str,
        requests: Vec<ImportPrivateKeyFromPathRequest>,
    ) -> Result<Vec<VaultScopedKeySummary>> {
        self.controller
            .import_private_keys_from_paths(vault_id, requests)
    }

    pub(crate) fn host_references_for_key(
        &self,
        vault_id: &str,
        key_id: &str,
    ) -> Result<Vec<HostReferenceSummary>> {
        self.controller.host_references_for_key(vault_id, key_id)
    }

    pub(crate) fn host_references_for_credential(
        &self,
        vault_id: &str,
        credential_id: &str,
    ) -> Result<Vec<HostReferenceSummary>> {
        self.controller
            .host_references_for_credential(vault_id, credential_id)
    }

    pub(crate) fn load_public_key(&self, vault_id: &str, key_id: &str) -> Result<Option<String>> {
        self.controller.load_public_key(vault_id, key_id)
    }

    pub(crate) fn generate_ed25519_key(
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

    pub(crate) fn generate_rsa_key(
        &self,
        vault_id: &str,
        label: String,
    ) -> Result<VaultScopedKeySummary> {
        self.generate_private_key(
            vault_id,
            GenerateKeyRequest {
                label,
                algorithm: GenerateKeyAlgorithm::Rsa { bits: 4096 },
            },
        )
    }

    pub(crate) fn build_connect_request(
        &self,
        vault_id: &str,
        host_id: &str,
    ) -> Result<SshConnectRequest> {
        self.controller.build_connect_request(vault_id, host_id)
    }

    pub(crate) fn build_port_forward_request(
        &self,
        vault_id: &str,
        port_forward_id: &str,
    ) -> Result<SshPortForwardRequest> {
        self.controller
            .build_port_forward_request(vault_id, port_forward_id)
    }

    pub(crate) fn start_port_forward(
        &self,
        request: SshPortForwardRequest,
    ) -> Result<PortForwardRuntimeSnapshot> {
        self.controller.start_port_forward(request)
    }

    pub(crate) fn stop_port_forward(&self, id: &str) -> bool {
        self.controller.stop_port_forward(id)
    }

    pub(crate) fn sftp_canonicalize(&self, session_id: u64, path: &str) -> Result<String> {
        self.controller.sftp_canonicalize(session_id, path)
    }

    pub(crate) fn sftp_list_dir(&self, session_id: u64, path: &str) -> Result<Vec<SftpEntry>> {
        self.controller.sftp_list_dir(session_id, path)
    }

    pub(crate) fn sftp_read_file(&self, session_id: u64, remote_path: &str) -> Result<Vec<u8>> {
        self.controller.sftp_read_file(session_id, remote_path)
    }

    pub(crate) fn sftp_mkdir(&self, session_id: u64, path: &str) -> Result<()> {
        self.controller.sftp_mkdir(session_id, path)
    }

    pub(crate) fn sftp_remove(&self, session_id: u64, path: &str, is_dir: bool) -> Result<()> {
        self.controller.sftp_remove(session_id, path, is_dir)
    }

    pub(crate) fn sftp_rename(
        &self,
        session_id: u64,
        old_path: &str,
        new_path: &str,
    ) -> Result<()> {
        self.controller.sftp_rename(session_id, old_path, new_path)
    }
}
