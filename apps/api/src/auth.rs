//! Built-in auth: Argon2id passwords + TOTP RFC 6238 2FA + session cookies + API tokens.
//!
//! Flow:
//!   1. First run: `users` empty → UI shows /setup wizard to register first admin (no 2FA yet)
//!   2. Login: POST /auth/login with username+password → if `totp_enrolled`, session is "awaiting_2fa"
//!      Otherwise → session is full-access.
//!   3. POST /auth/verify-2fa with TOTP code → flips session out of awaiting_2fa.
//!   4. POST /auth/2fa/enrol → returns base32 secret + QR data URI to display.
//!   5. POST /auth/2fa/confirm with current code → flips `totp_enrolled = 1` + stores secret.
//!   6. POST /auth/2fa/recovery with a one-use code → falls back through if TOTP lost.

use crate::db::Db;
use anyhow::{anyhow, Context, Result};
use argon2::password_hash::{rand_core::OsRng, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Argon2, PasswordHash};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::FromRow;
use totp_rs::{Algorithm, Secret, TOTP};

#[derive(Debug, Clone, FromRow)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub password_hash: String,
    pub totp_secret_enc: Option<Vec<u8>>,
    pub totp_secret_nonce: Option<Vec<u8>>,
    pub totp_enrolled: i64,
    pub recovery_codes_enc: Option<Vec<u8>>,
    pub recovery_codes_nonce: Option<Vec<u8>>,
    pub is_admin: i64,
    pub created_at: i64,
    pub last_login_at: Option<i64>,
    /// Highest TOTP time-step already consumed — prevents replay within the validity window.
    pub last_totp_step: i64,
}

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| anyhow!("argon2 hash: {e}"))?
        .to_string();
    Ok(hash)
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(p) => p,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn hash_api_token(raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    B64.encode(h.finalize())
}

pub fn new_recovery_codes(n: usize) -> Vec<String> {
    (0..n)
        .map(|_| {
            // 10-char codes in xxxxx-xxxxx form
            let part = || -> String {
                rand::thread_rng()
                    .sample_iter(&Alphanumeric)
                    .take(5)
                    .map(char::from)
                    .map(|c| c.to_ascii_lowercase())
                    .collect()
            };
            format!("{}-{}", part(), part())
        })
        .collect()
}

pub fn make_totp(secret: &[u8], account_name: &str) -> Result<TOTP> {
    let totp = TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        secret.to_vec(),
        Some("tally".to_string()),
        account_name.to_string(),
    )
    .map_err(|e| anyhow!("totp init: {e}"))?;
    Ok(totp)
}

pub fn fresh_totp_secret() -> Vec<u8> {
    Secret::generate_secret().to_bytes().unwrap_or_default()
}

#[derive(Debug, Deserialize)]
pub struct LoginInput {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Deserialize)]
pub struct VerifyTotpInput {
    pub code: String,
}

pub async fn count_users(db: &Db) -> Result<i64> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&db.pool)
        .await?;
    Ok(row.0)
}

pub async fn create_user(db: &Db, username: &str, password: &str) -> Result<i64> {
    let hash = hash_password(password)?;
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO users (username, password_hash, created_at) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(username)
    .bind(hash)
    .bind(chrono::Utc::now().timestamp())
    .fetch_one(&db.pool)
    .await
    .context("create user")?;
    Ok(row.0)
}

pub async fn find_user_by_username(db: &Db, username: &str) -> Result<Option<User>> {
    Ok(sqlx::query_as::<_, User>(
        "SELECT * FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(&db.pool)
    .await?)
}

pub async fn save_totp_secret(
    db: &Db,
    user_id: i64,
    secret: &[u8],
    recovery_codes: &[String],
) -> Result<()> {
    let secret_b64 = B64.encode(secret);
    let (s_nonce, s_ct) = db.crypto.encrypt(&secret_b64)?;
    let codes_json = serde_json::to_string(recovery_codes)?;
    let (c_nonce, c_ct) = db.crypto.encrypt(&codes_json)?;
    sqlx::query(
        "UPDATE users SET totp_secret_enc = ?, totp_secret_nonce = ?,
            recovery_codes_enc = ?, recovery_codes_nonce = ?, totp_enrolled = 1
         WHERE id = ?",
    )
    .bind(&s_ct)
    .bind(&s_nonce)
    .bind(&c_ct)
    .bind(&c_nonce)
    .bind(user_id)
    .execute(&db.pool)
    .await?;
    Ok(())
}

pub fn decrypt_totp_secret(db: &Db, user: &User) -> Result<Vec<u8>> {
    let (Some(nonce), Some(ct)) = (&user.totp_secret_nonce, &user.totp_secret_enc) else {
        return Err(anyhow!("user has no TOTP enrolment"));
    };
    let decoded_b64 = db.crypto.decrypt(nonce, ct)?;
    Ok(B64.decode(decoded_b64).context("base64 decode TOTP secret")?)
}

pub fn decrypt_recovery_codes(db: &Db, user: &User) -> Result<Vec<String>> {
    let (Some(nonce), Some(ct)) = (&user.recovery_codes_nonce, &user.recovery_codes_enc) else {
        return Ok(vec![]);
    };
    let json = db.crypto.decrypt(nonce, ct)?;
    Ok(serde_json::from_str(&json)?)
}

pub async fn consume_recovery_code(db: &Db, user: &User, candidate: &str) -> Result<bool> {
    let mut codes = decrypt_recovery_codes(db, user)?;
    let candidate = candidate.trim().to_lowercase();
    if let Some(pos) = codes.iter().position(|c| c.eq_ignore_ascii_case(&candidate)) {
        codes.remove(pos);
        // Re-encrypt + save remaining codes.
        let json = serde_json::to_string(&codes)?;
        let (nonce, ct) = db.crypto.encrypt(&json)?;
        sqlx::query(
            "UPDATE users SET recovery_codes_enc = ?, recovery_codes_nonce = ? WHERE id = ?",
        )
        .bind(&ct)
        .bind(&nonce)
        .bind(user.id)
        .execute(&db.pool)
        .await?;
        Ok(true)
    } else {
        Ok(false)
    }
}

pub fn verify_totp(secret: &[u8], code: &str, account_name: &str) -> bool {
    match make_totp(secret, account_name) {
        Ok(totp) => totp.check_current(code.trim()).unwrap_or(false),
        Err(_) => false,
    }
}

/// Verify a login TOTP against the user's stored secret AND enforce single-use: a code is
/// accepted only if its time-step is strictly newer than the last consumed step, then that step
/// is recorded. This blocks replay of an intercepted code within its ±1-step validity window.
pub async fn verify_totp_fresh(db: &Db, user: &User, code: &str) -> Result<bool> {
    let Ok(secret) = decrypt_totp_secret(db, user) else {
        return Ok(false);
    };
    let Ok(totp) = make_totp(&secret, &user.username) else {
        return Ok(false);
    };
    let code = code.trim();
    let now = chrono::Utc::now().timestamp().max(0) as u64;
    let step = now / 30;
    // Accept the current step and ±1 skew (matching make_totp's skew of 1).
    for cand in [step.saturating_sub(1), step, step + 1] {
        if totp.generate(cand * 30) == code {
            if (cand as i64) <= user.last_totp_step {
                return Ok(false); // already-used or older step → replay
            }
            sqlx::query("UPDATE users SET last_totp_step = ? WHERE id = ?")
                .bind(cand as i64)
                .bind(user.id)
                .execute(&db.pool)
                .await?;
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn touch_last_login(db: &Db, user_id: i64) -> Result<()> {
    sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
        .bind(chrono::Utc::now().timestamp())
        .bind(user_id)
        .execute(&db.pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Crypto;
    use crate::db::Db;
    use base64::engine::general_purpose::STANDARD as B64STD;

    async fn fetch_user(db: &Db, id: i64) -> User {
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
            .bind(id)
            .fetch_one(&db.pool)
            .await
            .unwrap()
    }

    /// In-memory DB with migrations applied. `max_connections(1)` keeps the single shared
    /// `:memory:` database alive across queries.
    async fn test_db() -> Db {
        let crypto = Crypto::from_b64(&B64STD.encode([7u8; 32])).unwrap();
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        Db { pool, crypto }
    }

    #[test]
    fn password_hash_roundtrip() {
        let h = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &h));
        assert!(!verify_password("wrong", &h));
    }

    #[tokio::test]
    async fn totp_code_cannot_be_replayed() {
        let db = test_db().await;
        let uid = create_user(&db, "alice", "a-strong-password").await.unwrap();
        let secret = fresh_totp_secret();
        save_totp_secret(&db, uid, &secret, &new_recovery_codes(2))
            .await
            .unwrap();

        let user = fetch_user(&db, uid).await;
        let totp = make_totp(&secret, &user.username).unwrap();
        let now = chrono::Utc::now().timestamp() as u64;
        let code = totp.generate((now / 30) * 30);

        // First presentation succeeds and records the step.
        assert!(verify_totp_fresh(&db, &user, &code).await.unwrap());

        // Same code, replayed against the now-updated user → rejected.
        let user_after = fetch_user(&db, uid).await;
        assert!(user_after.last_totp_step > 0);
        assert!(
            !verify_totp_fresh(&db, &user_after, &code).await.unwrap(),
            "a TOTP code must not be accepted twice"
        );

        // A wrong code is rejected.
        assert!(!verify_totp_fresh(&db, &user_after, "000000").await.unwrap());
    }
}
