//! Repro for the production panic: `libsql ... i64::from_sql -> unreachable!("invalid value type")`.
//! Hypothesis: `spending_by_category`'s `SUM(amount_cents)` read as `get::<i64>` panics when a
//! group's SUM comes back as a REAL (float) — which happens if ANY summed row is a float, even one
//! outside the dashboard's normal read window. Pure-integer groups should be fine; a mixed group
//! that yields a REAL should reproduce the panic. Then we prove the CAST/Value-coercion fix.

use libsql::{params, Builder, Connection, Value};

async fn seed(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE transactions (
            id INTEGER PRIMARY KEY,
            amount_cents INTEGER NOT NULL,
            is_credit INTEGER NOT NULL,
            category_id INTEGER,
            timestamp INTEGER NOT NULL
        );
        -- integer amounts (normal)
        INSERT INTO transactions (amount_cents, is_credit, category_id, timestamp) VALUES (1000, 0, 1, 100);
        INSERT INTO transactions (amount_cents, is_credit, category_id, timestamp) VALUES (2000, 0, 1, 100);",
    )
    .await
    .unwrap();
    // A float slips into the INTEGER-affinity column (non-lossless => stored as REAL). This mirrors
    // a real-world row the importer/CSV path produced. Same category group as the ints above.
    conn.execute(
        "INSERT INTO transactions (amount_cents, is_credit, category_id, timestamp) VALUES (?1, 0, 1, 100)",
        params![1234.5_f64],
    )
    .await
    .unwrap();
}

/// The CURRENT (buggy) read: `SUM(...)` as get::<i64>. Expected to panic when the SUM is REAL.
#[tokio::test]
#[should_panic(expected = "invalid value type")]
async fn current_read_panics_on_real_sum() {
    let db = Builder::new_local(":memory:").build().await.unwrap();
    let conn = db.connect().unwrap();
    seed(&conn).await;
    let mut rows = conn
        .query(
            "SELECT category_id, SUM(amount_cents) AS total_cents
             FROM transactions WHERE is_credit = 0 GROUP BY category_id",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let _cat: Option<i64> = row.get(0).unwrap();
    let _sum: i64 = row.get(1).unwrap(); // <-- panics here when SUM is REAL
}

/// The FIX: cast the aggregate to INTEGER in SQL so libsql always sees an Integer value.
#[tokio::test]
async fn cast_fix_never_panics() {
    let db = Builder::new_local(":memory:").build().await.unwrap();
    let conn = db.connect().unwrap();
    seed(&conn).await;
    let mut rows = conn
        .query(
            "SELECT category_id, CAST(COALESCE(SUM(amount_cents), 0) AS INTEGER) AS total_cents
             FROM transactions WHERE is_credit = 0 GROUP BY category_id",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let _cat: Option<i64> = row.get(0).unwrap();
    let sum: i64 = row.get(1).unwrap();
    assert_eq!(sum, 4234); // 1000 + 2000 + trunc(1234.5) = 4234
}

/// Belt-and-braces: a Rust-side coercion that tolerates Integer/Real/Null without SQL changes.
#[tokio::test]
async fn value_coercion_never_panics() {
    let db = Builder::new_local(":memory:").build().await.unwrap();
    let conn = db.connect().unwrap();
    seed(&conn).await;
    let mut rows = conn
        .query(
            "SELECT SUM(amount_cents) AS total_cents FROM transactions WHERE is_credit = 0",
            (),
        )
        .await
        .unwrap();
    let row = rows.next().await.unwrap().unwrap();
    let v = row.get_value(0).unwrap();
    let sum = match v {
        Value::Integer(i) => i,
        Value::Real(f) => f as i64,
        Value::Null => 0,
        _ => 0,
    };
    assert_eq!(sum, 4234);
}
