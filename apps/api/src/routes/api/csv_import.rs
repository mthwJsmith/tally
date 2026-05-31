//! CSV importer.
//!
//! Two-step flow:
//!   1. POST /api/csv/preview  — upload raw CSV bytes; server detects columns, returns suggested mapping
//!   2. POST /api/csv/commit   — apply mapping + account selection, batch-insert transactions
//!
//! Dedup: `provider_txn_id` is set to `csv:<sha256-of-row>` so re-uploads are idempotent.

use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::NaiveDate;
use csv::ReaderBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct PreviewBody {
    /// Raw CSV text. Frontend reads the file as a string and posts it here.
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct PreviewReply {
    pub headers: Vec<String>,
    /// Up to 5 sample rows for the UI to render a mapping preview.
    pub sample_rows: Vec<Vec<String>>,
    pub row_count: usize,
    pub suggested: SuggestedMapping,
}

#[derive(Debug, Serialize, Default)]
pub struct SuggestedMapping {
    pub date_column: Option<String>,
    pub description_column: Option<String>,
    pub amount_column: Option<String>,
    pub credit_column: Option<String>,
    pub debit_column: Option<String>,
    pub balance_column: Option<String>,
}

pub async fn preview(Json(body): Json<PreviewBody>) -> Result<Json<PreviewReply>, (StatusCode, String)> {
    let mut rdr = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(body.content.as_bytes());
    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("bad CSV headers: {e}")))?
        .iter()
        .map(|s| s.to_string())
        .collect();
    let mut sample_rows = Vec::new();
    let mut count = 0;
    for (i, rec) in rdr.records().enumerate() {
        let rec = rec.map_err(|e| (StatusCode::BAD_REQUEST, format!("bad CSV row {i}: {e}")))?;
        if sample_rows.len() < 5 {
            sample_rows.push(rec.iter().map(|s| s.to_string()).collect());
        }
        count += 1;
    }
    let suggested = suggest_mapping(&headers);
    Ok(Json(PreviewReply {
        headers,
        sample_rows,
        row_count: count,
        suggested,
    }))
}

#[derive(Debug, Deserialize)]
pub struct CommitBody {
    pub content: String,
    pub account_id: i64,
    pub mapping: CommitMapping,
    pub currency: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CommitMapping {
    pub date_column: String,
    pub description_column: String,
    /// Either a single amount column (positive = credit, negative = debit)
    /// OR separate credit + debit columns.
    pub amount_column: Option<String>,
    pub credit_column: Option<String>,
    pub debit_column: Option<String>,
    /// Date format string (chrono syntax), e.g. "%d/%m/%Y" for Nationwide.
    /// Defaults try ISO 8601, then DD/MM/YYYY, then MM/DD/YYYY.
    pub date_format: Option<String>,
}

pub async fn commit(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CommitBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let mut rdr = ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(body.content.as_bytes());
    let headers: Vec<String> = rdr
        .headers()
        .map_err(bad)?
        .iter()
        .map(|s| s.to_string())
        .collect();

    let idx = |name: &str| -> Result<usize, (StatusCode, String)> {
        headers
            .iter()
            .position(|h| h.eq_ignore_ascii_case(name))
            .ok_or((StatusCode::BAD_REQUEST, format!("column '{name}' not in CSV")))
    };
    let date_i = idx(&body.mapping.date_column)?;
    let desc_i = idx(&body.mapping.description_column)?;
    let amount_i = body.mapping.amount_column.as_deref().map(idx).transpose()?;
    let credit_i = body.mapping.credit_column.as_deref().map(idx).transpose()?;
    let debit_i = body.mapping.debit_column.as_deref().map(idx).transpose()?;

    if amount_i.is_none() && credit_i.is_none() && debit_i.is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            "either amount_column OR (credit_column + debit_column) required".into(),
        ));
    }

    let currency = body.currency.unwrap_or_else(|| "GBP".to_string());
    let mut imported = 0i64;
    let mut skipped = 0i64;
    let mut errors: Vec<String> = Vec::new();

    for (row_n, rec) in rdr.records().enumerate() {
        let Ok(rec) = rec else {
            errors.push(format!("row {row_n}: parse failed"));
            continue;
        };
        let raw_row: String = rec.iter().collect::<Vec<_>>().join("\u{1f}");
        let mut h = Sha256::new();
        h.update(raw_row.as_bytes());
        let hash = format!(
            "csv:{}:{:x}",
            body.account_id,
            h.finalize()
        );

        let date_str = rec.get(date_i).unwrap_or("").trim();
        let desc = rec.get(desc_i).unwrap_or("").trim().to_string();
        let ts = parse_date(date_str, body.mapping.date_format.as_deref());
        let Some(ts) = ts else {
            errors.push(format!("row {row_n}: couldn't parse date '{date_str}'"));
            continue;
        };

        let (is_credit, amount_cents) = if let Some(ai) = amount_i {
            let v = rec.get(ai).unwrap_or("0").trim().replace(",", "").replace("£", "");
            let n: f64 = v.parse().unwrap_or(0.0);
            (n >= 0.0, (n.abs() * 100.0).round() as i64)
        } else {
            let c = credit_i
                .and_then(|i| rec.get(i))
                .map(|s| s.trim().replace(",", "").replace("£", ""))
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            let d = debit_i
                .and_then(|i| rec.get(i))
                .map(|s| s.trim().replace(",", "").replace("£", ""))
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            if c > 0.0 {
                (true, (c * 100.0).round() as i64)
            } else if d > 0.0 {
                (false, (d * 100.0).round() as i64)
            } else {
                skipped += 1;
                continue;
            }
        };

        if state.db.is_txn_seen(body.account_id, &hash).await.unwrap_or(false) {
            skipped += 1;
            continue;
        }

        let new_id = match state
            .db
            .upsert_transaction(
                body.account_id,
                &hash,
                ts,
                &desc,
                amount_cents,
                &currency,
                is_credit,
                false,
                None,
                None,
                None,
                None,
                None,
                None,
            )
            .await
        {
            Ok(id) => id,
            Err(e) => {
                errors.push(format!("row {row_n}: db error {e:#}"));
                continue;
            }
        };
        let _ = state
            .db
            .record_txn_imported(body.account_id, &hash, Some(new_id), false, None)
            .await;
        imported += 1;
    }

    Ok(Json(json!({
        "imported": imported,
        "skipped": skipped,
        "errors": errors,
    })))
}

fn parse_date(s: &str, explicit_fmt: Option<&str>) -> Option<i64> {
    if let Some(fmt) = explicit_fmt {
        if let Ok(d) = NaiveDate::parse_from_str(s, fmt) {
            return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp());
        }
    }
    for fmt in &["%Y-%m-%d", "%d/%m/%Y", "%m/%d/%Y", "%d-%m-%Y", "%d %b %Y", "%d %B %Y"] {
        if let Ok(d) = NaiveDate::parse_from_str(s, fmt) {
            return Some(d.and_hms_opt(0, 0, 0)?.and_utc().timestamp());
        }
    }
    // Try RFC3339 with time
    if let Ok(d) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(d.timestamp());
    }
    None
}

fn suggest_mapping(headers: &[String]) -> SuggestedMapping {
    let find = |patterns: &[&str]| -> Option<String> {
        headers
            .iter()
            .find(|h| {
                let lower = h.to_lowercase();
                patterns.iter().any(|p| lower.contains(p))
            })
            .cloned()
    };
    SuggestedMapping {
        date_column: find(&["date", "transaction date"]),
        description_column: find(&["description", "narrative", "details", "transaction"]),
        amount_column: find(&["amount", "value"]),
        credit_column: find(&["credit", "money in", "paid in", "deposit"]),
        debit_column: find(&["debit", "money out", "paid out", "withdrawal"]),
        balance_column: find(&["balance"]),
    }
}

fn bad<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, format!("{e}"))
}
