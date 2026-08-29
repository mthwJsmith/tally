//! Schema migrations for the libsql backend, replacing `sqlx::migrate!`.
//!
//! Version-presence only: a migration is applied iff its version is absent from the
//! `_sqlx_migrations` ledger with `success = 1`. Checksums are intentionally NEVER compared —
//! that structurally eliminates the historical "migration N was previously applied but has
//! been modified" crash class. The ledger keeps a schema compatible with sqlx's, so a database
//! already migrated by the old sqlx path (e.g. the production Pi, versions 1..12) reports every
//! version applied and re-runs nothing.

use anyhow::{Context, Result};
use libsql::Connection;

struct Migration {
    version: i64,
    name: &'static str,
    sql: &'static str,
}

/// All migrations, embedded in the binary so the container needs no migrations directory.
const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "init", sql: include_str!("../migrations/0001_init.sql") },
    Migration { version: 2, name: "full_features", sql: include_str!("../migrations/0002_full_features.sql") },
    Migration { version: 3, name: "auth", sql: include_str!("../migrations/0003_auth.sql") },
    Migration { version: 4, name: "ai_settings", sql: include_str!("../migrations/0004_ai_settings.sql") },
    Migration { version: 5, name: "holdings", sql: include_str!("../migrations/0005_holdings.sql") },
    Migration { version: 6, name: "balances_renames", sql: include_str!("../migrations/0006_balances_renames.sql") },
    Migration { version: 7, name: "oauth", sql: include_str!("../migrations/0007_oauth.sql") },
    Migration { version: 8, name: "drop_legacy_auth", sql: include_str!("../migrations/0008_drop_legacy_auth.sql") },
    Migration { version: 9, name: "drop_api_tokens", sql: include_str!("../migrations/0009_drop_api_tokens.sql") },
    Migration { version: 10, name: "reminders", sql: include_str!("../migrations/0010_reminders.sql") },
    Migration { version: 11, name: "watchlist", sql: include_str!("../migrations/0011_watchlist.sql") },
    Migration { version: 12, name: "drop_watchlist", sql: include_str!("../migrations/0012_drop_watchlist.sql") },
    Migration { version: 13, name: "planning", sql: include_str!("../migrations/0013_planning.sql") },
    Migration { version: 14, name: "balance_snapshots", sql: include_str!("../migrations/0014_balance_snapshots.sql") },
    Migration { version: 15, name: "floor_overflow", sql: include_str!("../migrations/0015_floor_overflow.sql") },
    Migration { version: 16, name: "mcp_views", sql: include_str!("../migrations/0016_mcp_views.sql") },
    Migration { version: 17, name: "fx_holdings_view", sql: include_str!("../migrations/0017_fx_holdings_view.sql") },
    Migration { version: 18, name: "retirement", sql: include_str!("../migrations/0018_retirement.sql") },
    Migration { version: 19, name: "net_worth_history", sql: include_str!("../migrations/0019_net_worth_history.sql") },
];

const LEDGER_DDL: &str = "CREATE TABLE IF NOT EXISTS _sqlx_migrations (
    version BIGINT PRIMARY KEY,
    description TEXT NOT NULL,
    installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    success BOOLEAN NOT NULL,
    checksum BLOB NOT NULL,
    execution_time BIGINT NOT NULL
);";

/// Apply every embedded migration whose version is not yet recorded as successful.
pub async fn run(conn: &Connection) -> Result<()> {
    conn.execute_batch(LEDGER_DDL)
        .await
        .context("create _sqlx_migrations ledger")?;

    let mut applied = std::collections::HashSet::new();
    let mut rows = conn
        .query("SELECT version FROM _sqlx_migrations WHERE success = 1", ())
        .await
        .context("read migration ledger")?;
    while let Some(row) = rows.next().await? {
        applied.insert(row.get::<i64>(0)?);
    }

    let pending: Vec<&Migration> = MIGRATIONS
        .iter()
        .filter(|m| !applied.contains(&m.version))
        .collect();
    if pending.is_empty() {
        tracing::info!("migrations: up to date ({} applied)", applied.len());
        return Ok(());
    }

    // PRAGMA foreign_keys is a no-op inside a transaction, so toggle it here in autocommit.
    // Migrations that rebuild tables can transiently violate FKs; the app connection runs FK-on.
    conn.execute_batch("PRAGMA foreign_keys=OFF;").await.ok();

    for m in pending {
        tracing::info!("migrations: applying {} ({})", m.version, m.name);
        let tx = conn.transaction().await.context("begin migration tx")?;
        tx.execute_batch(m.sql)
            .await
            .with_context(|| format!("migration {} ({}) failed", m.version, m.name))?;
        tx.execute(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (?1, ?2, 1, ?3, 0)",
            libsql::params![m.version, m.name, Vec::<u8>::new()],
        )
        .await
        .context("record migration in ledger")?;
        tx.commit().await.context("commit migration tx")?;
    }

    conn.execute_batch("PRAGMA foreign_keys=ON;").await.ok();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use libsql::Builder;

    async fn mem() -> Connection {
        let db = Builder::new_local(":memory:").build().await.unwrap();
        db.connect().unwrap()
    }

    async fn applied_versions(conn: &Connection) -> Vec<i64> {
        let mut rows = conn
            .query("SELECT version FROM _sqlx_migrations WHERE success=1 ORDER BY version", ())
            .await
            .unwrap();
        let mut v = Vec::new();
        while let Some(r) = rows.next().await.unwrap() {
            v.push(r.get::<i64>(0).unwrap());
        }
        v
    }

    async fn app_table_count(conn: &Connection) -> i64 {
        let mut rows = conn
            .query(
                "SELECT count(*) FROM sqlite_master WHERE type='table' \
                 AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations'",
                (),
            )
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get::<i64>(0).unwrap()
    }

    #[tokio::test]
    async fn fresh_db_applies_all() {
        let conn = mem().await;
        run(&conn).await.unwrap();
        assert_eq!(applied_versions(&conn).await, (1..=19).collect::<Vec<_>>());
        // The real migration SQL produced real tables under libsql (dialect smoke check).
        assert!(app_table_count(&conn).await > 0);
    }

    #[tokio::test]
    async fn rerun_is_idempotent() {
        let conn = mem().await;
        run(&conn).await.unwrap();
        run(&conn).await.unwrap(); // second run must be a no-op, not an error
        assert_eq!(applied_versions(&conn).await, (1..=19).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn mcp_views_are_selectable() {
        // Preparing a SELECT against each reporting view fails if any referenced column
        // drifts from the real schema — cheap guard for the MCP `query` tool surface.
        let conn = mem().await;
        run(&conn).await.unwrap();
        for v in [
            "v_accounts", "v_transactions", "v_plan_accounts", "v_plan_events", "v_bills",
            "v_goals", "v_holdings", "v_categories", "v_net_worth_history",
        ] {
            conn.query(&format!("SELECT * FROM {v} LIMIT 1"), ())
                .await
                .unwrap_or_else(|e| panic!("view {v} broken: {e}"));
        }
        // The MCP `query` tool wraps user SQL as `SELECT * FROM (<sql>) LIMIT n` — make sure
        // the wrapper also accepts a WITH…SELECT inside the subquery.
        conn.query(
            "SELECT * FROM (WITH m AS (SELECT 1 AS x) SELECT * FROM m) LIMIT 201",
            (),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn preseeded_ledger_runs_nothing() {
        // Simulate the production Pi: ledger records every version as applied (with a non-empty
        // checksum that must never be read), but the tables do NOT exist. A version-presence
        // runner must skip them all; a checksum-reading runner would crash, and a re-running one
        // would build tables. We assert no app table was created.
        let conn = mem().await;
        conn.execute_batch(LEDGER_DDL).await.unwrap();
        for m in MIGRATIONS {
            conn.execute(
                "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
                 VALUES (?1, 'x', 1, ?2, 0)",
                libsql::params![m.version, vec![0xde_u8, 0xad]],
            )
            .await
            .unwrap();
        }
        run(&conn).await.unwrap();
        assert_eq!(app_table_count(&conn).await, 0, "no migration should have run");
    }
}
