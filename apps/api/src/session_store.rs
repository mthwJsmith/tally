//! A `tower_sessions::SessionStore` backed by the same libsql connection as the rest of the app
//! (replaces `tower-sessions-sqlx-store::SqliteStore`, which depends on sqlx and so cannot link
//! alongside libsql).
//!
//! Storage shape: a single `tower_sessions` table —
//!   `id TEXT PRIMARY KEY, data BLOB NOT NULL, expiry_date INTEGER NOT NULL`.
//! The whole `Record` is serialized to JSON bytes for the `data` column (so all session keys
//! round-trip), keyed by `Id::to_string()`. `expiry_date` is stored as a Unix timestamp so we
//! can prune / reject expired rows on load.

use std::sync::Arc;

use async_trait::async_trait;
use libsql::{params, Connection};
use time::OffsetDateTime;
use tower_sessions::session::{Id, Record};
use tower_sessions::session_store::{self, SessionStore};

#[derive(Clone)]
pub struct LibsqlSessionStore {
    conn: Arc<Connection>,
}

impl std::fmt::Debug for LibsqlSessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibsqlSessionStore").finish()
    }
}

impl LibsqlSessionStore {
    pub fn new(conn: Arc<Connection>) -> Self {
        Self { conn }
    }

    /// Create the backing table if it doesn't already exist. Call once at startup.
    pub async fn migrate(&self) -> session_store::Result<()> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS tower_sessions (
                    id TEXT PRIMARY KEY,
                    data BLOB NOT NULL,
                    expiry_date INTEGER NOT NULL
                );",
            )
            .await
            .map_err(backend)?;
        Ok(())
    }

    async fn upsert(&self, record: &Record) -> session_store::Result<()> {
        let data = serde_json::to_vec(record).map_err(|e| session_store::Error::Encode(e.to_string()))?;
        let id = record.id.to_string();
        let expiry = record.expiry_date.unix_timestamp();
        self.conn
            .execute(
                "INSERT INTO tower_sessions (id, data, expiry_date) VALUES (?1, ?2, ?3)
                 ON CONFLICT(id) DO UPDATE SET data = excluded.data, expiry_date = excluded.expiry_date",
                params![id, data, expiry],
            )
            .await
            .map_err(backend)?;
        Ok(())
    }

    /// Whether the given session id is already present (used by `create` for collision checks).
    async fn id_exists(&self, id: &Id) -> session_store::Result<bool> {
        let mut rows = self
            .conn
            .query(
                "SELECT 1 FROM tower_sessions WHERE id = ?1",
                params![id.to_string()],
            )
            .await
            .map_err(backend)?;
        Ok(rows.next().await.map_err(backend)?.is_some())
    }
}

fn backend(e: libsql::Error) -> session_store::Error {
    session_store::Error::Backend(e.to_string())
}

#[async_trait]
impl SessionStore for LibsqlSessionStore {
    async fn create(&self, record: &mut Record) -> session_store::Result<()> {
        // Mitigate id collisions: regenerate until the id is free, then insert.
        while self.id_exists(&record.id).await? {
            record.id = Id::default();
        }
        self.upsert(record).await
    }

    async fn save(&self, record: &Record) -> session_store::Result<()> {
        self.upsert(record).await
    }

    async fn load(&self, session_id: &Id) -> session_store::Result<Option<Record>> {
        let mut rows = self
            .conn
            .query(
                "SELECT data, expiry_date FROM tower_sessions WHERE id = ?1",
                params![session_id.to_string()],
            )
            .await
            .map_err(backend)?;
        let Some(row) = rows.next().await.map_err(backend)? else {
            return Ok(None);
        };
        let expiry = row.get::<i64>(1).map_err(backend)?;
        // Expired sessions are treated as absent (and pruned best-effort).
        if expiry <= OffsetDateTime::now_utc().unix_timestamp() {
            let _ = self.delete(session_id).await;
            return Ok(None);
        }
        let data: Vec<u8> = row.get(0).map_err(backend)?;
        // A row we cannot decode is treated as no session (pruned best-effort) rather than an
        // error: this is what gracefully retires sessions written by the previous sqlx store
        // (MessagePack) after the cutover — users simply re-log in once, no request failures.
        match serde_json::from_slice::<Record>(&data) {
            Ok(record) => Ok(Some(record)),
            Err(_) => {
                let _ = self.delete(session_id).await;
                Ok(None)
            }
        }
    }

    async fn delete(&self, session_id: &Id) -> session_store::Result<()> {
        self.conn
            .execute(
                "DELETE FROM tower_sessions WHERE id = ?1",
                params![session_id.to_string()],
            )
            .await
            .map_err(backend)?;
        Ok(())
    }
}
