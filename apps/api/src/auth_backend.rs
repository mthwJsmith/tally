//! axum-login backend: bridges tally's `users` table to axum-login / tower-sessions.
//!
//! Credentials are still verified with Argon2id (`auth::verify_password`); axum-login owns
//! the session lifecycle (cookie, store, rehydration) so we no longer hand-roll a `sessions`
//! table or session tokens. TOTP 2FA is layered on top as a partial-auth step in
//! `routes/login.rs`: on first-factor success for a 2FA-enrolled user we stash a pending
//! user id in the tower-session instead of calling `AuthSession::login`.

use crate::auth::{self, User};
use crate::db::Db;
use async_trait::async_trait;
use axum_login::{AuthUser, AuthnBackend, UserId};

impl AuthUser for User {
    type Id = i64;

    fn id(&self) -> i64 {
        self.id
    }

    fn session_auth_hash(&self) -> &[u8] {
        // Tie the session to the password hash so changing the password invalidates
        // all existing sessions.
        self.password_hash.as_bytes()
    }
}

#[derive(Clone)]
pub struct Backend {
    db: Db,
}

impl Backend {
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

#[derive(Clone)]
pub struct Credentials {
    pub username: String,
    pub password: String,
}

#[async_trait]
impl AuthnBackend for Backend {
    type User = User;
    type Credentials = Credentials;
    type Error = sqlx::Error;

    async fn authenticate(&self, creds: Credentials) -> Result<Option<User>, Self::Error> {
        let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE username = ?")
            .bind(creds.username.trim())
            .fetch_optional(&self.db.pool)
            .await?;
        // Constant-ish: only return the user when the password verifies.
        Ok(user.filter(|u| auth::verify_password(&creds.password, &u.password_hash)))
    }

    async fn get_user(&self, user_id: &UserId<Self>) -> Result<Option<User>, Self::Error> {
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
            .bind(*user_id)
            .fetch_optional(&self.db.pool)
            .await
    }
}

/// Convenience alias used by handlers and middleware.
pub type AuthSession = axum_login::AuthSession<Backend>;
