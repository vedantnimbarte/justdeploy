//! Secrets at rest: env values, git tokens, backup credentials.

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key, Nonce,
};
use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::{distributions::Alphanumeric, Rng};

const NONCE_LEN: usize = 12;

pub struct Cipher(Aes256Gcm);

impl Cipher {
    pub fn new(key: &[u8; 32]) -> Self {
        Self(Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key)))
    }

    /// base64(nonce || ciphertext)
    pub fn encrypt(&self, plaintext: &str) -> String {
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = self
            .0
            .encrypt(&nonce, plaintext.as_bytes())
            .expect("AES-GCM encryption cannot fail for in-memory input");
        let mut out = nonce.to_vec();
        out.extend_from_slice(&ciphertext);
        STANDARD.encode(out)
    }

    pub fn decrypt(&self, encoded: &str) -> Result<String> {
        let bytes = STANDARD.decode(encoded)?;
        if bytes.len() < NONCE_LEN {
            return Err(anyhow!("ciphertext too short"));
        }
        let (nonce, ciphertext) = bytes.split_at(NONCE_LEN);
        let plaintext = self
            .0
            .decrypt(Nonce::from_slice(nonce), ciphertext)
            .map_err(|_| anyhow!("cannot decrypt a stored secret: was JD_MASTER_KEY changed?"))?;
        Ok(String::from_utf8(plaintext)?)
    }
}

/// Random alphanumeric string, for generated passwords and webhook secrets.
pub fn random_string(len: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(len)
        .map(char::from)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_uses_a_fresh_nonce() {
        let cipher = Cipher::new(&[7; 32]);
        let a = cipher.encrypt("hunter2");
        let b = cipher.encrypt("hunter2");
        assert_ne!(a, b, "same plaintext must not produce the same ciphertext");
        assert_eq!(cipher.decrypt(&a).unwrap(), "hunter2");
    }

    #[test]
    fn wrong_key_is_an_error_not_garbage() {
        let a = Cipher::new(&[1; 32]).encrypt("secret");
        assert!(Cipher::new(&[2; 32]).decrypt(&a).is_err());
    }

    #[test]
    fn random_strings_have_the_requested_length() {
        assert_eq!(random_string(24).len(), 24);
    }
}
