use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use rand::RngCore;
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::{VaultError, VaultResult};

const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;

#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub(crate) struct SecretKey(pub(crate) [u8; KEY_LEN]);

impl SecretKey {
    pub(crate) fn generate() -> Self {
        let mut key = [0_u8; KEY_LEN];
        rand::thread_rng().fill_bytes(&mut key);
        Self(key)
    }

    pub(crate) fn from_slice(bytes: &[u8]) -> VaultResult<Self> {
        if bytes.len() != KEY_LEN {
            return Err(VaultError::InvalidKeyLength {
                expected: KEY_LEN,
                actual: bytes.len(),
            });
        }

        let mut key = [0_u8; KEY_LEN];
        key.copy_from_slice(bytes);
        Ok(Self(key))
    }

    pub(crate) fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CipherEnvelope {
    pub(crate) nonce: Vec<u8>,
    pub(crate) ciphertext: Vec<u8>,
}

pub(crate) fn encrypt(
    key: &SecretKey,
    plaintext: &[u8],
    aad: &[u8],
) -> VaultResult<CipherEnvelope> {
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_bytes())
        .map_err(|_| VaultError::CipherInitFailed)?;

    let mut nonce = [0_u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce);

    let ciphertext = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|source| VaultError::EncryptionFailed { source })?;

    Ok(CipherEnvelope {
        nonce: nonce.to_vec(),
        ciphertext,
    })
}

pub(crate) fn decrypt(
    key: &SecretKey,
    nonce: &[u8],
    ciphertext: &[u8],
    aad: &[u8],
) -> VaultResult<Zeroizing<Vec<u8>>> {
    if nonce.len() != NONCE_LEN {
        return Err(VaultError::InvalidNonceLength {
            expected: NONCE_LEN,
            actual: nonce.len(),
        });
    }

    let cipher = XChaCha20Poly1305::new_from_slice(key.as_bytes())
        .map_err(|_| VaultError::CipherInitFailed)?;

    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|source| VaultError::DecryptionFailed { source })
}
