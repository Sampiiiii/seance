use std::sync::Arc;

use anyhow::Result;
use seance_ssh::{ResolvedAuthMethod, SshConnectRequest, SshConnectionConfig, SshSessionManager};
use seance_terminal::{LocalSessionFactory, TerminalSession};
use seance_vault::{
    CredentialSummary, GenerateKeyAlgorithm, GenerateKeyRequest, HostAuthRef, HostSummary,
    ImportKeyRequest, KeySummary, SecretString, VaultHostProfile, VaultPasswordCredential,
    VaultStatus, VaultStore,
};

pub struct UiBackend {
    vault: VaultStore,
    ssh: SshSessionManager,
    local: LocalSessionFactory,
}

impl UiBackend {
    pub fn new(vault: VaultStore) -> Result<Self> {
        Ok(Self {
            vault,
            ssh: SshSessionManager::new()?,
            local: LocalSessionFactory::default(),
        })
    }

    pub fn vault_status(&self) -> VaultStatus {
        self.vault.status()
    }

    pub fn try_unlock_with_device(&mut self) -> Result<bool> {
        Ok(self.vault.try_unlock_with_device()?)
    }

    pub fn create_vault(&mut self, passphrase: &SecretString, device_name: &str) -> Result<()> {
        Ok(self.vault.create_vault(passphrase, device_name)?)
    }

    pub fn unlock_vault(&mut self, passphrase: &SecretString, device_name: &str) -> Result<()> {
        Ok(self.vault.unlock_with_passphrase(passphrase, device_name)?)
    }

    pub fn lock_vault(&mut self) {
        self.vault.lock();
    }

    pub fn spawn_local_session(&self) -> Result<Arc<dyn TerminalSession>> {
        Ok(Arc::new(self.local.spawn()?))
    }

    pub fn list_hosts(&self) -> Result<Vec<HostSummary>> {
        Ok(self.vault.list_host_profiles()?)
    }

    pub fn load_host(&self, id: &str) -> Result<Option<VaultHostProfile>> {
        Ok(self.vault.load_host_profile(id)?)
    }

    pub fn save_host(&mut self, host: VaultHostProfile) -> Result<HostSummary> {
        Ok(self.vault.store_host_profile(host)?)
    }

    pub fn delete_host(&mut self, id: &str) -> Result<bool> {
        Ok(self.vault.delete_host_profile(id)?)
    }

    pub fn list_password_credentials(&self) -> Result<Vec<CredentialSummary>> {
        Ok(self.vault.list_password_credentials()?)
    }

    pub fn load_password_credential(
        &self,
        id: &str,
    ) -> Result<Option<VaultPasswordCredential>> {
        Ok(self.vault.load_password_credential(id)?)
    }

    pub fn save_password_credential(
        &mut self,
        credential: VaultPasswordCredential,
    ) -> Result<CredentialSummary> {
        Ok(self.vault.store_password_credential(credential)?)
    }

    pub fn delete_password_credential(&mut self, id: &str) -> Result<bool> {
        Ok(self.vault.delete_password_credential(id)?)
    }

    pub fn list_private_keys(&self) -> Result<Vec<KeySummary>> {
        Ok(self.vault.list_private_keys()?)
    }

    #[allow(dead_code)]
    pub fn import_private_key(&mut self, request: ImportKeyRequest) -> Result<KeySummary> {
        Ok(self.vault.import_private_key(request)?)
    }

    pub fn delete_private_key(&mut self, id: &str) -> Result<bool> {
        Ok(self.vault.delete_private_key(id)?)
    }

    pub fn generate_private_key(&mut self, request: GenerateKeyRequest) -> Result<KeySummary> {
        Ok(self.vault.generate_private_key(request)?)
    }

    pub fn generate_ed25519_key(&mut self, label: String) -> Result<KeySummary> {
        self.generate_private_key(GenerateKeyRequest {
            label,
            algorithm: GenerateKeyAlgorithm::Ed25519,
        })
    }

    pub fn generate_rsa_key(&mut self, label: String) -> Result<KeySummary> {
        self.generate_private_key(GenerateKeyRequest {
            label,
            algorithm: GenerateKeyAlgorithm::Rsa { bits: 4096 },
        })
    }

    pub fn connect_host(&mut self, host_id: &str) -> Result<Arc<dyn TerminalSession>> {
        let host = self
            .vault
            .load_host_profile(host_id)?
            .ok_or_else(|| anyhow::anyhow!("saved host not found"))?;

        let mut auth_order = Vec::with_capacity(host.auth_order.len());
        for auth in &host.auth_order {
            match auth {
                HostAuthRef::Password { credential_id } => {
                    let credential = self
                        .vault
                        .load_password_credential(credential_id)?
                        .ok_or_else(|| anyhow::anyhow!("missing password credential"))?;
                    auth_order.push(ResolvedAuthMethod::Password {
                        password: credential.secret,
                    });
                }
                HostAuthRef::PrivateKey {
                    key_id,
                    passphrase_credential_id,
                } => {
                    let key = self
                        .vault
                        .load_private_key(key_id)?
                        .ok_or_else(|| anyhow::anyhow!("missing private key"))?;
                    let passphrase = passphrase_credential_id
                        .as_ref()
                        .map(|id| self.vault.load_password_credential(id))
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

        let result = self.ssh.connect(SshConnectRequest {
            connection: SshConnectionConfig {
                label: host.label,
                hostname: host.hostname,
                port: host.port,
                username: host.username,
            },
            auth_order,
        })?;

        Ok(result.session as Arc<dyn TerminalSession>)
    }
}
