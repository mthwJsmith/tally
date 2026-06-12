//! Standalone validation that all 12 real migrations apply cleanly under libsql, mirroring the
//! runner in src/migrate.rs. Lives in tests/ (libsql-only link unit) so it sidesteps the
//! sqlx/libsql duplicate-SQLite-symbol clash while the in-binary cutover is staged.

use libsql::{params, Builder, Connection};

const MIGRATIONS: &[(i64, &str, &str)] = &[
    (1, "init", include_str!("../migrations/0001_init.sql")),
    (2, "full_features", include_str!("../migrations/0002_full_features.sql")),
    (3, "auth", include_str!("../migrations/0003_auth.sql")),
    (4, "ai_settings", include_str!("../migrations/0004_ai_settings.sql")),
    (5, "holdings", include_str!("../migrations/0005_holdings.sql")),
    (6, "balances_renames", include_str!("../migrations/0006_balances_renames.sql")),
    (7, "oauth", include_str!("../migrations/0007_oauth.sql")),
    (8, "drop_legacy_auth", include_str!("../migrations/0008_drop_legacy_auth.sql")),
    (9, "drop_api_tokens", include_str!("../migrations/0009_drop_api_tokens.sql")),
    (10, "reminders", include_str!("../migrations/0010_reminders.sql")),
    (11, "watchlist", include_str!("../migrations/0011_watchlist.sql")),
    (12, "drop_watchlist", include_str!("../migrations/0012_drop_watchlist.sql")),
];

async fn run(conn: &Connection) -> anyhow::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _sqlx_migrations (version BIGINT PRIMARY KEY, \
         description TEXT NOT NULL, installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, \
         success BOOLEAN NOT NULL, checksum BLOB NOT NULL, execution_time BIGINT NOT NULL);",
    )
    .await?;
    let mut applied = std::collections::HashSet::new();
    let mut rows = conn
        .query("SELECT version FROM _sqlx_migrations WHERE success=1", ())
        .await?;
    while let Some(r) = rows.next().await? {
        applied.insert(r.get::<i64>(0)?);
    }
    conn.execute_batch("PRAGMA foreign_keys=OFF;").await.ok();
    for (v, name, sql) in MIGRATIONS {
        if applied.contains(v) {
            continue;
        }
        let tx = conn.transaction().await?;
        tx.execute_batch(sql).await?;
        tx.execute(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (?1, ?2, 1, ?3, 0)",
            params![*v, *name, Vec::<u8>::new()],
        )
        .await?;
        tx.commit().await?;
    }
    conn.execute_batch("PRAGMA foreign_keys=ON;").await.ok();
    Ok(())
}

async fn versions(conn: &Connection) -> Vec<i64> {
    let mut rows = conn
        .query("SELECT version FROM _sqlx_migrations WHERE success=1 ORDER BY version", ())
        .await
        .unwrap();
    let mut got = Vec::new();
    while let Some(r) = rows.next().await.unwrap() {
        got.push(r.get::<i64>(0).unwrap());
    }
    got
}

#[tokio::test]
async fn all_twelve_migrations_apply_under_libsql() {
    let db = Builder::new_local(":memory:").build().await.unwrap();
    let conn = db.connect().unwrap();

    run(&conn).await.unwrap();
    assert_eq!(versions(&conn).await, (1..=12).collect::<Vec<_>>());

    // The real migration SQL produced real tables under libsql's dialect.
    let mut rows = conn
        .query(
            "SELECT count(*) FROM sqlite_master WHERE type='table' \
             AND name NOT LIKE 'sqlite_%' AND name != '_sqlx_migrations'",
            (),
        )
        .await
        .unwrap();
    let n: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
    assert!(n > 0, "migrations should have created tables");

    // Idempotent re-run.
    run(&conn).await.unwrap();
    assert_eq!(versions(&conn).await, (1..=12).collect::<Vec<_>>());
}
