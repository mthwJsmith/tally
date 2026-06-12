//! Throwaway smoke test: validates the exact libsql 0.9 API shapes the db.rs rewrite
//! depends on (param binding, row-by-index, Option, BLOB, execute_batch, transactions,
//! column-name lookup). If this compiles and passes, the mass rewrite is safe to mirror.
//! Delete once the migration lands.

use libsql::{params, Builder};

#[tokio::test]
async fn libsql_core_api_smoke() {
    let db = Builder::new_local(":memory:").build().await.unwrap();
    let conn = db.connect().unwrap();

    // Multi-statement script — migrations run this way (execute_batch).
    conn.execute_batch(
        "CREATE TABLE t (id INTEGER PRIMARY KEY, name TEXT NOT NULL, note TEXT, amt REAL, blob BLOB);
         INSERT INTO t (name, note, amt, blob) VALUES ('seed', NULL, 1.5, x'00ff');",
    )
    .await
    .unwrap();

    // Parameterised insert (positional), with NULL via Option and a BLOB via Vec<u8>.
    conn.execute(
        "INSERT INTO t (name, note, amt, blob) VALUES (?1, ?2, ?3, ?4)",
        params![String::from("alice"), Option::<String>::None, 2.5_f64, vec![1u8, 2, 3]],
    )
    .await
    .unwrap();
    let new_id = conn.last_insert_rowid();
    assert!(new_id > 0, "last_insert_rowid should be positive");

    // Query + row access: i64 by index, String, Option<String> (NULL-safe), Option<f64>, Vec<u8>.
    let mut rows = conn
        .query("SELECT id, name, note, amt, blob FROM t ORDER BY id", ())
        .await
        .unwrap();
    let mut count = 0;
    let mut saw_null_note = false;
    let mut saw_blob = false;
    while let Some(row) = rows.next().await.unwrap() {
        let _id: i64 = row.get(0).unwrap();
        let _name: String = row.get(1).unwrap();
        let note: Option<String> = row.get(2).unwrap();
        let _amt: Option<f64> = row.get(3).unwrap();
        let blob: Vec<u8> = row.get(4).unwrap();
        if note.is_none() {
            saw_null_note = true;
        }
        if !blob.is_empty() {
            saw_blob = true;
        }
        count += 1;
    }
    assert_eq!(count, 2, "expected 2 rows");
    assert!(saw_null_note, "NULL note must map to None");
    assert!(saw_blob, "BLOB must map to non-empty Vec<u8>");

    // Column-name → index lookup (the Account/User mappers need presence-tolerant name access).
    let mut rows = conn.query("SELECT id, name FROM t WHERE name = ?1", params!["alice"]).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let n = row.column_count();
    let mut name_idx = None;
    for i in 0..n {
        if row.column_name(i) == Some("name") {
            name_idx = Some(i);
        }
    }
    let name_idx = name_idx.expect("column 'name' should be found by name");
    let alice: String = row.get(name_idx).unwrap();
    assert_eq!(alice, "alice");

    // Transaction: begin → execute → commit.
    let tx = conn.transaction().await.unwrap();
    tx.execute("UPDATE t SET amt = ?1 WHERE name = ?2", params![9.9_f64, "alice"]).await.unwrap();
    tx.commit().await.unwrap();

    let mut rows = conn.query("SELECT amt FROM t WHERE name = ?1", params!["alice"]).await.unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let amt: f64 = row.get(0).unwrap();
    assert!((amt - 9.9).abs() < 1e-9, "transaction commit should persist amt");
}
