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

pub async fn touch_last_login(db: &Db, user_id: i64) -> Result<()> {
    sqlx::query("UPDATE users SET last_login_at = ? WHERE id = ?")
        .bind(chrono::Utc::now().timestamp())
        .bind(user_id)
        .execute(&db.pool)
        .await?;
    Ok(())
}
