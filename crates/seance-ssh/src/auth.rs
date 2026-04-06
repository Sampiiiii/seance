use std::sync::Arc;

use russh::keys::{Algorithm as SshAlgorithm, PrivateKey as SshPrivateKey, PublicKey};
use russh::{client, keys::PrivateKeyWithHashAlg};

use crate::model::{ResolvedAuthMethod, SshError};

#[derive(Default)]
pub(crate) struct AcceptAnyHostKeyHandler;

impl client::Handler for AcceptAnyHostKeyHandler {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &PublicKey,
    ) -> std::result::Result<bool, Self::Error> {
        Ok(true)
    }
}

pub(crate) async fn authenticate(
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
