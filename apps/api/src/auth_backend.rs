//! axum-login backend: bridges tally's `users` table to axum-login / tower-sessions.
//!
//! Credentials are still verified with Argon2id (`auth::verify_password`); axum-login owns
//! the session lifecycle (cookie, store, rehydration) so we no longer hand-roll a `sessions`
//! table or session tokens. TOTP 2FA is layered on top as a partial-auth step in
//! `routes/login.rs`: on first-factor success for a 2FA-enrolled user we stash a pending
//! user id in the tower-session instead of calling `AuthSession::login`.

use crate::auth::{self, User};
use crate::db::Db;
use crate::models::FromLibsqlRow;
use axum_login::{AuthUser, AuthnBackend, UserId};
use libsql::params;

/// axum-login requires the backend's associated `Error` to be `std::error::Error + Send + Sync +
/// 'static`. `anyhow::Error` does NOT impl `std::error::Error`, so wrap it in a thiserror enum
/// that does. All libsql/db failures funnel through `Db(anyhow::Error)`.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error(transparent)]
    Db(#[from] anyhow::Error),
}

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

impl AuthnBackend for Backend {
    type User = User;
    type Credentials = Credentials;
    type Error = AuthError;

    async fn authenticate(&self, creds: Credentials) -> Result<Option<User>, Self::Error> {
        let mut rows = self
            .db
            .conn
            .query(
                "SELECT * FROM users WHERE username = ?1",
                params![creds.username.trim()],
            )
            .await
            .map_err(anyhow::Error::from)?;
        let user = match rows.next().await.map_err(anyhow::Error::from)? {
            Some(row) => Some(User::from_row(&row)?),
            None => None,
        };
        // Burn the same Argon2 work whether or not the username exists so response
        // timing doesn't reveal which usernames are registered.
        match user {
            Some(u) => Ok(Some(u).filter(|u2| auth::verify_password(&creds.password, &u2.password_hash))),
            None => {
                auth::verify_password_dummy(&creds.password);
                Ok(None)
            }
        }
    }

    async fn get_user(&self, user_id: &UserId<Self>) -> Result<Option<User>, Self::Error> {
        let mut rows = self
            .db
            .conn
            .query("SELECT * FROM users WHERE id = ?1", params![*user_id])
            .await
            .map_err(anyhow::Error::from)?;
        match rows.next().await.map_err(anyhow::Error::from)? {
            Some(row) => Ok(Some(User::from_row(&row)?)),
            None => Ok(None),
        }
    }
}

/// Convenience alias used by handlers and middleware.
pub type AuthSession = axum_login::AuthSession<Backend>;
