//! Built-in auth: Argon2id passwords + TOTP RFC 6238 2FA + session cookies.
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
use crate::models::{ColumnIndex, FromLibsqlRow};
use anyhow::{anyhow, Context, Result};
use argon2::password_hash::{rand_core::OsRng, PasswordHasher, PasswordVerifier, SaltString};
use argon2::{Argon2, PasswordHash};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as B64, Engine};
use libsql::params;
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::Deserialize;
use totp_rs::{Algorithm, Secret, TOTP};

#[derive(Debug, Clone)]
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

impl FromLibsqlRow for User {
    fn from_row(row: &libsql::Row) -> anyhow::Result<Self> {
        // Loaded via SELECT * — map strictly by column name. The BLOB columns are nullable
        // encrypted secrets (Option<Vec<u8>>); they must round-trip, hence a manual mapper.
        let c = ColumnIndex::new(row);
        Ok(Self {
            id: { let i = c.req("id")?; row.get(i)? },
            username: { let i = c.req("username")?; row.get(i)? },
            password_hash: { let i = c.req("password_hash")?; row.get(i)? },
            totp_secret_enc: { let i = c.req("totp_secret_enc")?; row.get(i)? },
            totp_secret_nonce: { let i = c.req("totp_secret_nonce")?; row.get(i)? },
            totp_enrolled: { let i = c.req("totp_enrolled")?; row.get(i)? },
            recovery_codes_enc: { let i = c.req("recovery_codes_enc")?; row.get(i)? },
            recovery_codes_nonce: { let i = c.req("recovery_codes_nonce")?; row.get(i)? },
            is_admin: { let i = c.req("is_admin")?; row.get(i)? },
            created_at: { let i = c.req("created_at")?; row.get(i)? },
            last_login_at: { let i = c.req("last_login_at")?; row.get(i)? },
            last_totp_step: { let i = c.req("last_totp_step")?; row.get(i)? },
        })
    }
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

/// A real Argon2id hash that non-existent users are "verified" against so login latency
/// doesn't reveal whether a username is registered.
static DUMMY_HASH: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    hash_password("tally-timing-equalizer").expect("argon2 dummy hash")
});

/// Burn the same Argon2 work as a real verification; always fails.
pub fn verify_password_dummy(password: &str) {
    let _ = verify_password(password, &DUMMY_HASH);
}

/// Constant-time equality for short secrets (recovery / TOTP codes). Length still
/// short-circuits, which is fine — code lengths are public.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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
    let mut rows = db.conn.query("SELECT COUNT(*) FROM users", ()).await?;
    let row = rows.next().await?.ok_or_else(|| anyhow!("count produced no row"))?;
    Ok(row.get::<i64>(0)?)
}

pub async fn create_user(db: &Db, username: &str, password: &str) -> Result<i64> {
    let hash = hash_password(password)?;
    let mut rows = db
        .conn
        .query(
            "INSERT INTO users (username, password_hash, created_at) VALUES (?1, ?2, ?3) RETURNING id",
            params![username, hash, chrono::Utc::now().timestamp()],
        )
        .await?;
    let row = rows
        .next()
        .await?
        .ok_or_else(|| anyhow!("create user RETURNING produced no row"))?;
    Ok(row.get::<i64>(0)?)
}

pub async fn find_user_by_id(db: &Db, id: i64) -> Result<Option<User>> {
    let mut rows = db
        .conn
        .query("SELECT * FROM users WHERE id = ?1", params![id])
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(User::from_row(&row)?)),
        None => Ok(None),
    }
}

pub async fn find_user_by_username(db: &Db, username: &str) -> Result<Option<User>> {
    let mut rows = db
        .conn
        .query("SELECT * FROM users WHERE username = ?1", params![username])
        .await?;
    match rows.next().await? {
        Some(row) => Ok(Some(User::from_row(&row)?)),
        None => Ok(None),
    }
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
    db.conn
        .execute(
            "UPDATE users SET totp_secret_enc = ?1, totp_secret_nonce = ?2,
                recovery_codes_enc = ?3, recovery_codes_nonce = ?4, totp_enrolled = 1
             WHERE id = ?5",
            params![s_ct, s_nonce, c_ct, c_nonce, user_id],
        )
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
    if let Some(pos) = codes
        .iter()
        .position(|c| ct_eq(c.to_lowercase().as_bytes(), candidate.as_bytes()))
    {
        codes.remove(pos);
        // Re-encrypt + save remaining codes.
        let json = serde_json::to_string(&codes)?;
        let (nonce, ct) = db.crypto.encrypt(&json)?;
        db.conn
            .execute(
                "UPDATE users SET recovery_codes_enc = ?1, recovery_codes_nonce = ?2 WHERE id = ?3",
                params![ct, nonce, user.id],
            )
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
        if ct_eq(totp.generate(cand * 30).as_bytes(), code.as_bytes()) {
            if (cand as i64) <= user.last_totp_step {
                return Ok(false); // already-used or older step → replay
            }
            db.conn
                .execute(
                    "UPDATE users SET last_totp_step = ?1 WHERE id = ?2",
                    params![cand as i64, user.id],
                )
                .await?;
            return Ok(true);
        }
    }
    Ok(false)
}

pub async fn touch_last_login(db: &Db, user_id: i64) -> Result<()> {
    db.conn
        .execute(
            "UPDATE users SET last_login_at = ?1 WHERE id = ?2",
            params![chrono::Utc::now().timestamp(), user_id],
        )
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
        let mut rows = db
            .conn
            .query("SELECT * FROM users WHERE id = ?1", params![id])
            .await
            .unwrap();
        let row = rows.next().await.unwrap().unwrap();
        User::from_row(&row).unwrap()
    }

    /// In-memory libsql DB with migrations applied. A single connection keeps the shared
    /// `:memory:` database alive across queries.
    async fn test_db() -> Db {
        let crypto = Crypto::from_b64(&B64STD.encode([7u8; 32])).unwrap();
        let database = libsql::Builder::new_local(":memory:")
            .build()
            .await
            .unwrap();
        let conn = database.connect().unwrap();
        crate::migrate::run(&conn).await.unwrap();
        Db {
            conn: std::sync::Arc::new(conn),
            crypto,
        }
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
