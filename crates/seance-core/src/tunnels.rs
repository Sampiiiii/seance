// Owns vault-scoped tunnel models and SSH request construction for port forwards.

use anyhow::{Result, anyhow};
use seance_ssh::{
    ResolvedAuthMethod, SshConnectRequest, SshConnectionConfig, SshPortForwardMode,
    SshPortForwardRequest,
};
use seance_vault::{
    HostAuthRef, PortForwardMode, PortForwardSummary, VaultHostProfile, VaultPortForwardProfile,
    VaultStore,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VaultScopedPortForwardSummary {
    pub vault_id: String,
    pub vault_name: String,
    pub host_id: String,
    pub host_label: String,
    pub port_forward: PortForwardSummary,
}

pub(crate) fn port_forward_scope_key(vault_id: &str, port_forward_id: &str) -> String {
    format!("{vault_id}::{port_forward_id}")
}

pub(crate) fn build_connect_request(
    store: &VaultStore,
    host: &VaultHostProfile,
) -> Result<SshConnectRequest> {
    let mut auth_order = Vec::with_capacity(host.auth_order.len());
    for auth in &host.auth_order {
        match auth {
            HostAuthRef::Password { credential_id } => {
                let credential = store
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
                let key = store
                    .load_private_key(key_id)?
                    .ok_or_else(|| anyhow!("missing private key"))?;
                let passphrase = passphrase_credential_id
                    .as_ref()
                    .map(|id| store.load_password_credential(id))
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
            label: host.label.clone(),
            hostname: host.hostname.clone(),
            port: host.port,
            username: host.username.clone(),
        },
        auth_order,
    })
}

pub(crate) fn build_port_forward_request(
    store: &VaultStore,
    vault_id: &str,
    port_forward: VaultPortForwardProfile,
) -> Result<SshPortForwardRequest> {
    let host = store
        .load_host_profile(&port_forward.host_id)?
        .ok_or_else(|| anyhow!("saved host not found"))?;
    let connect_request = build_connect_request(store, &host)?;

    Ok(SshPortForwardRequest {
        id: port_forward_scope_key(vault_id, &port_forward.id),
        vault_id: vault_id.to_string(),
        forward_id: port_forward.id,
        host_id: host.id.clone(),
        label: port_forward.label,
        host_label: host.label.clone(),
        mode: match port_forward.mode {
            PortForwardMode::Local => SshPortForwardMode::Local,
            PortForwardMode::Remote => SshPortForwardMode::Remote,
        },
        listen_address: port_forward.listen_address,
        listen_port: port_forward.listen_port,
        target_address: port_forward.target_address,
        target_port: port_forward.target_port,
        connection: connect_request.connection,
        auth_order: connect_request.auth_order,
    })
}
