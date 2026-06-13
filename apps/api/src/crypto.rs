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
use sha2::{Digest, Sha256};

#[derive(Clone)]
pub struct Crypto {
    cipher: ChaCha20Poly1305,
}

impl Crypto {
    pub fn from_env() -> Result<Self> {
        let raw = std::env::var("TALLY_MASTER_KEY").context("TALLY_MASTER_KEY env var not set")?;
        let raw = raw.trim();
        // Preferred form: a 32-byte base64 key (`openssl rand -base64 32`), used VERBATIM so
        // existing deployments stay byte-for-byte identical. Otherwise — a Cloud Run
        // auto-generated secret, or any other string — derive a 32-byte key by hashing it, so a
        // one-click deploy works without hand-generating a key. The input must be high-entropy:
        // a guessable passphrase weakens the at-rest encryption, since the key is never stored
        // and an attacker with the database would otherwise have to brute-force the input.
        if let Ok(bytes) = B64.decode(raw) {
            if bytes.len() == 32 {
                return Self::from_key_bytes(&bytes);
            }
        }
        Self::from_key_bytes(&Sha256::digest(raw.as_bytes()))
    }

    /// Build from a base64-encoded 32-byte key. Used by tests.
    pub fn from_b64(raw: &str) -> Result<Self> {
        let bytes = B64
            .decode(raw.trim())
            .context("TALLY_MASTER_KEY is not valid base64")?;
        Self::from_key_bytes(&bytes)
    }

    /// Build the cipher from exactly 32 key bytes.
    fn from_key_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 32 {
            return Err(anyhow!(
                "master key must be exactly 32 bytes (got {})",
                bytes.len()
            ));
        }
        let key = Key::from_slice(bytes);
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

    // One test (not two) so the TALLY_MASTER_KEY env mutations can't race each other.
    #[test]
    fn from_env_key_derivation() {
        // 1) A Cloud Run auto-generated secret (not a 32-byte base64 key) still produces a
        //    working cipher, and deriving it twice yields the SAME key — else data would become
        //    undecryptable after a restart.
        std::env::set_var("TALLY_MASTER_KEY", "cloud run generated secret! not base64");
        let a = Crypto::from_env().unwrap();
        let (nonce, ct) = a.encrypt("balance: 12345").unwrap();
        let b = Crypto::from_env().unwrap();
        assert_eq!(b.decrypt(&nonce, &ct).unwrap(), "balance: 12345");

        // 2) Back-compat: a 32-byte base64 key is used VERBATIM (not hashed), so existing
        //    encrypted data stays readable. from_env and from_b64 must agree on the key.
        let b64 = B64.encode([7u8; 32]);
        std::env::set_var("TALLY_MASTER_KEY", &b64);
        let env = Crypto::from_env().unwrap();
        let direct = Crypto::from_b64(&b64).unwrap();
        let (n2, c2) = direct.encrypt("secret").unwrap();
        assert_eq!(env.decrypt(&n2, &c2).unwrap(), "secret");

        std::env::remove_var("TALLY_MASTER_KEY");
    }
}
