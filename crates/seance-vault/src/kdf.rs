use argon2::{Algorithm, Argon2, Params, Version};
use rand::RngCore;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use crate::{VaultError, VaultResult, crypto::SecretKey};

pub const KDF_SALT_LEN: usize = 16;
pub const WRAP_KEY_LEN: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct KdfParams {
    pub salt: Vec<u8>,
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

impl KdfParams {
    pub fn recommended() -> Self {
        let mut salt = vec![0_u8; KDF_SALT_LEN];
        rand::thread_rng().fill_bytes(&mut salt);

        Self {
            salt,
            memory_kib: 131_072,
            iterations: 4,
            parallelism: 4,
        }
    }

    pub fn derive_wrap_key(&self, passphrase: &SecretString) -> VaultResult<SecretKey> {
        let params = Params::new(
            self.memory_kib,
            self.iterations,
            self.parallelism,
            Some(WRAP_KEY_LEN),
        )
        .map_err(|source| VaultError::InvalidKdfConfig {
            message: source.to_string(),
        })?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut output = [0_u8; WRAP_KEY_LEN];

        argon
            .hash_password_into(
                passphrase.expose_secret().as_bytes(),
                &self.salt,
                &mut output,
            )
            .map_err(|source| VaultError::PassphraseDerivationFailed {
                message: source.to_string(),
            })?;

        Ok(SecretKey(output))
    }
}
