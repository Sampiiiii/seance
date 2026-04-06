use std::sync::Arc;

use anyhow::Result;
use seance_core::{AppControllerHandle, SessionId, SessionKind};
use seance_ssh::{SftpEntry, SshConnectRequest, SshSessionManager};
use seance_terminal::TerminalSession;
use seance_vault::{
    CredentialSummary, GenerateKeyAlgorithm, GenerateKeyRequest, HostSummary, ImportKeyRequest,
    KeySummary, SecretString, VaultHostProfile, VaultPasswordCredential, VaultStatus,
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

    pub fn ssh_manager(&self) -> Arc<SshSessionManager> {
        self.controller.ssh_manager()
    }

    pub fn vault_status(&self) -> VaultStatus {
        self.controller.vault_status()
    }

    #[allow(dead_code)]
    pub fn try_unlock_with_device(&self) -> Result<bool> {
        self.controller.try_unlock_with_device()
    }

    pub fn create_vault(&self, passphrase: &SecretString, device_name: &str) -> Result<()> {
        self.controller.create_vault(passphrase, device_name)
    }

    pub fn unlock_vault(&self, passphrase: &SecretString, device_name: &str) -> Result<()> {
        self.controller.unlock_vault(passphrase, device_name)
    }

    pub fn lock_vault(&self) {
        self.controller.lock_vault();
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

    pub fn list_hosts(&self) -> Result<Vec<HostSummary>> {
        self.controller.list_hosts()
    }

    pub fn load_host(&self, id: &str) -> Result<Option<VaultHostProfile>> {
        self.controller.load_host(id)
    }

    pub fn save_host(&self, host: VaultHostProfile) -> Result<HostSummary> {
        self.controller.save_host(host)
    }

    pub fn delete_host(&self, id: &str) -> Result<bool> {
        self.controller.delete_host(id)
    }

    pub fn list_password_credentials(&self) -> Result<Vec<CredentialSummary>> {
        self.controller.list_password_credentials()
    }

    pub fn load_password_credential(&self, id: &str) -> Result<Option<VaultPasswordCredential>> {
        self.controller.load_password_credential(id)
    }

    pub fn save_password_credential(
        &self,
        credential: VaultPasswordCredential,
    ) -> Result<CredentialSummary> {
        self.controller.save_password_credential(credential)
    }

    pub fn delete_password_credential(&self, id: &str) -> Result<bool> {
        self.controller.delete_password_credential(id)
    }

    pub fn list_private_keys(&self) -> Result<Vec<KeySummary>> {
        self.controller.list_private_keys()
    }

    #[allow(dead_code)]
    pub fn import_private_key(&self, _request: ImportKeyRequest) -> Result<KeySummary> {
        anyhow::bail!("private key import is not yet wired through the resident controller")
    }

    pub fn delete_private_key(&self, id: &str) -> Result<bool> {
        self.controller.delete_private_key(id)
    }

    pub fn generate_private_key(&self, request: GenerateKeyRequest) -> Result<KeySummary> {
        self.controller.generate_private_key(request)
    }

    pub fn generate_ed25519_key(&self, label: String) -> Result<KeySummary> {
        self.generate_private_key(GenerateKeyRequest {
            label,
            algorithm: GenerateKeyAlgorithm::Ed25519,
        })
    }

    pub fn generate_rsa_key(&self, label: String) -> Result<KeySummary> {
        self.generate_private_key(GenerateKeyRequest {
            label,
            algorithm: GenerateKeyAlgorithm::Rsa { bits: 4096 },
        })
    }

    pub fn build_connect_request(&self, host_id: &str) -> Result<SshConnectRequest> {
        self.controller.build_connect_request(host_id)
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
