//! ChaCha20-Poly1305 encryption for OAuth tokens at rest.
//!
//! Master key is derived from the `TALLY_MASTER_KEY` env var (must be 32 bytes, base64-encoded).
//! Generate one with: `openssl rand -base64 32`
//!
//! Threat model: protects against SSD theft + offline read of `state.db`. Does NOT protect
//! against attackers who already have root on the running container (the key is in process memory).

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chacha20poly1305::{
    aead::{Aead, KeyInit, OsRng},
    AeadCore, ChaCha20Poly1305, Key, Nonce,
};

#[derive(Clone)]
pub struct Crypto {
    cipher: ChaCha20Poly1305,
}

impl Crypto {
    pub fn from_env() -> Result<Self> {
        let raw = std::env::var("TALLY_MASTER_KEY")
            .context("TALLY_MASTER_KEY env var not set — generate with `openssl rand -base64 32`")?;
        Self::from_b64(raw.trim())
    }

    /// Build from a base64-encoded 32-byte key. Used by `from_env` and by tests.
    pub fn from_b64(raw: &str) -> Result<Self> {
        let bytes = B64
            .decode(raw.trim())
            .context("TALLY_MASTER_KEY is not valid base64")?;
        if bytes.len() != 32 {
            return Err(anyhow!(
                "TALLY_MASTER_KEY must decode to exactly 32 bytes (got {})",
                bytes.len()
            ));
        }
        let key = Key::from_slice(&bytes);
        Ok(Self {
            cipher: ChaCha20Poly1305::new(key),
        })
    }

    /// Encrypt a UTF-8 string. Returns `(nonce, ciphertext)`.
    pub fn encrypt(&self, plaintext: &str) -> Result<(Vec<u8>, Vec<u8>)> {
        let nonce = ChaCha20Poly1305::generate_nonce(&mut OsRng);
        let ct = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|e| anyhow!("encrypt failed: {e}"))?;
        Ok((nonce.to_vec(), ct))
    }

    /// Decrypt back to UTF-8 string.
    pub fn decrypt(&self, nonce: &[u8], ciphertext: &[u8]) -> Result<String> {
        if nonce.len() != 12 {
            return Err(anyhow!("nonce must be 12 bytes, got {}", nonce.len()));
        }
        let nonce = Nonce::from_slice(nonce);
        let pt = self
            .cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| anyhow!("decrypt failed: {e}"))?;
        String::from_utf8(pt).context("decrypted bytes not valid UTF-8")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let c = Crypto::from_b64(&B64.encode([0u8; 32])).unwrap();
        let (nonce, ct) = c.encrypt("hello world").unwrap();
        assert_eq!(c.decrypt(&nonce, &ct).unwrap(), "hello world");
    }

    #[test]
    fn rejects_wrong_key_length() {
        assert!(Crypto::from_b64(&B64.encode([0u8; 16])).is_err());
    }
}
