//! "Safe to spend" — forward-looking runway to the next payday.
//!
//! Mirrors the frontend calc in `apps/web/src/lib/safe-to-spend.ts` so the dashboard tile, the
//! `get_safe_to_spend` MCP tool and the daily Telegram ping all agree. Config is persisted as a
//! JSON blob in the existing `settings` key/value table (no new migration).
//!
//! The headline idea is the per-account FLOOR: money below an account's floor is never counted as
//! spendable. A current account defaults to a £0 floor; an arranged-overdraft account can have a
//! negative floor (a 0% buffer line), and that floor can flip on a CLIFF date (e.g. the day a 0%
//! overdraft ends and interest starts).

use crate::models::Bill;
use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use chrono::{Datelike, Duration, NaiveDate, Utc, Weekday};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const CONFIG_KEY: &str = "safe_to_spend_config";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Payday {
    LastWorkingDay,
    DayOfMonth { day: u32 },
}
impl Default for Payday {
    fn default() -> Self {
        Payday::LastWorkingDay
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cliff {
    pub account_id: i64,
    /// ISO date (YYYY-MM-DD). On/after this date, the account's floor becomes new_floor_cents.
    pub date_iso: String,
    pub new_floor_cents: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    pub payday: Payday,
    #[serde(default)]
    pub floors_cents: HashMap<i64, i64>,
    #[serde(default)]
    pub cliffs: Vec<Cliff>,
    #[serde(default)]
    pub ringfence_cents: i64,
    #[serde(default)]
    pub configured: bool,
}

// ---- date + floor helpers ---------------------------------------------------------------------

fn last_day_of_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    (NaiveDate::from_ymd_opt(ny, nm, 1).unwrap() - Duration::days(1)).day()
}

/// Last Mon–Fri of the given month.
fn last_working_day(year: i32, month: u32) -> NaiveDate {
    let last = last_day_of_month(year, month);
    let mut d = NaiveDate::from_ymd_opt(year, month, last).unwrap();
    while matches!(d.weekday(), Weekday::Sat | Weekday::Sun) {
        d -= Duration::days(1);
    }
    d
}

/// The next payday strictly after `today` (a payday falling on today rolls to next month).
fn next_payday(cfg: &Config, today: NaiveDate) -> NaiveDate {
    let build = |year: i32, month: u32| -> NaiveDate {
        match &cfg.payday {
            Payday::LastWorkingDay => last_working_day(year, month),
            Payday::DayOfMonth { day } => {
                let clamped = (*day).min(last_day_of_month(year, month)).max(1);
                NaiveDate::from_ymd_opt(year, month, clamped).unwrap()
            }
        }
    };
    let this = build(today.year(), today.month());
    if this > today {
        return this;
    }
    let (ny, nm) = if today.month() == 12 {
        (today.year() + 1, 1)
    } else {
        (today.year(), today.month() + 1)
    };
    build(ny, nm)
}

/// Active floor (cents) for an account, applying the latest cliff that has already passed.
fn floor_for(cfg: &Config, account_id: i64, today: NaiveDate) -> i64 {
    let mut floor = cfg.floors_cents.get(&account_id).copied().unwrap_or(0);
    let mut best: Option<NaiveDate> = None;
    for c in &cfg.cliffs {
        if c.account_id != account_id {
            continue;
        }
        if let Ok(d) = NaiveDate::parse_from_str(&c.date_iso, "%Y-%m-%d") {
            if d <= today && best.map_or(true, |b| d > b) {
                best = Some(d);
                floor = c.new_floor_cents;
            }
        }
    }
    floor
}

/// Committed direct-debit / bill outflows between `from_ts` and `payday_ts`, deduped by name
/// (stripping a trailing " (DD)" suffix, like the dashboard does).
fn committed_before_payday(bills: &[Bill], from_ts: i64, payday_ts: i64) -> i64 {
    let mut seen = HashSet::new();
    let mut total = 0i64;
    for b in bills {
        let Some(ned) = b.next_expected_date else { continue };
        if ned < from_ts || ned > payday_ts {
            continue;
        }
        let mut n = b.name.trim_end();
        if let Some(stripped) = n.strip_suffix("(DD)") {
            n = stripped.trim_end();
        }
        if !seen.insert(n.to_lowercase()) {
            continue;
        }
        total += if b.expected_amount_max_cents > 0 {
            b.expected_amount_max_cents
        } else {
            b.expected_amount_min_cents
        };
    }
    total
}

// ---- the calculation --------------------------------------------------------------------------

async fn load_config(state: &AppState) -> anyhow::Result<Config> {
    match state.db.get_setting(CONFIG_KEY).await? {
        Some(s) => Ok(serde_json::from_str(&s).unwrap_or_default()),
        None => Ok(Config::default()),
    }
}

/// Compute the safe-to-spend snapshot. Shared by the REST handler, the MCP tool and the scheduler.
/// Returns a camelCase JSON object matching the frontend `SafeToSpendResult`.
pub async fn compute(state: &AppState) -> anyhow::Result<Value> {
    let cfg = load_config(state).await?;
    let accounts = state.db.list_all_enabled_accounts().await?;
    let bills = state.db.list_bills().await?;

    let now = Utc::now();
    let today = now.date_naive();
    let now_ts = now.timestamp();
    let today_start = today.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();

    let spend_rows = state.db.spending_by_category(today_start, now_ts).await?;
    let today_spent: i64 = spend_rows.iter().map(|(_, c)| *c).sum();

    let payday = next_payday(&cfg, today);
    let payday_ts = payday.and_hms_opt(23, 59, 59).unwrap().and_utc().timestamp();
    let days_left = (payday - today).num_days().max(1);

    let mut per_account = Vec::new();
    let mut spendable_now = 0i64;
    for a in &accounts {
        if a.kind != "account" {
            continue;
        }
        let Some(cur) = a.current_balance_cents else { continue };
        let floor = floor_for(&cfg, a.id, today);
        let spendable = (cur - floor).max(0);
        spendable_now += spendable;
        let name = a
            .custom_display_name
            .clone()
            .or_else(|| a.consent_nickname.clone())
            .unwrap_or_else(|| a.display_name.clone());
        per_account.push(json!({
            "accountId": a.id,
            "name": name,
            "currentCents": cur,
            "floorCents": floor,
            "spendableCents": spendable,
        }));
    }

    let committed = committed_before_payday(&bills, today_start, payday_ts);
    let free = spendable_now - committed - cfg.ringfence_cents;
    let safe_per_day = (free as f64 / days_left as f64).floor() as i64;
    let safe_today = safe_per_day - today_spent;

    Ok(json!({
        "safeTodayCents": safe_today,
        "safePerDayCents": safe_per_day,
        "spendableNowCents": spendable_now,
        "committedCents": committed,
        "ringfenceCents": cfg.ringfence_cents,
        "freeCents": free,
        "daysLeft": days_left,
        "spentTodayCents": today_spent,
        "nextPaydayUnix": payday.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
        "configured": cfg.configured,
        "perAccount": per_account,
    }))
}

// ---- HTTP handlers ----------------------------------------------------------------------------

pub async fn get_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    Ok(Json(compute(&state).await.map_err(internal)?))
}

pub async fn get_config(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let cfg = load_config(&state).await.map_err(internal)?;
    Ok(Json(serde_json::to_value(cfg).map_err(internal)?))
}

pub async fn put_config(
    State(state): State<Arc<AppState>>,
    Json(cfg): Json<Config>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let s = serde_json::to_string(&cfg).map_err(internal)?;
    state.db.set_setting(CONFIG_KEY, &s).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        let mut floors = HashMap::new();
        floors.insert(1, 0); // Nationwide floor £0
        floors.insert(2, -100_000); // Santander floor -£1000
        Config {
            payday: Payday::LastWorkingDay,
            floors_cents: floors,
            cliffs: vec![Cliff {
                account_id: 2,
                date_iso: "2026-07-15".into(),
                new_floor_cents: 0,
            }],
            ringfence_cents: 0,
            configured: true,
        }
    }

    #[test]
    fn overdrawn_account_headroom_above_buffer() {
        // An account at -£832.32 against a -£1000 floor leaves £167.68 spendable — the
        // arranged overdraft counts as headroom only down to the floor, not to zero.
        let today = NaiveDate::from_ymd_opt(2026, 6, 20).unwrap();
        assert_eq!(floor_for(&cfg(), 2, today), -100_000);
        let spendable = (-83_232i64 - floor_for(&cfg(), 2, today)).max(0);
        assert_eq!(spendable, 16_768);
    }

    #[test]
    fn cliff_flips_floor_to_zero() {
        let before = NaiveDate::from_ymd_opt(2026, 7, 14).unwrap();
        let after = NaiveDate::from_ymd_opt(2026, 7, 15).unwrap();
        assert_eq!(floor_for(&cfg(), 2, before), -100_000);
        assert_eq!(floor_for(&cfg(), 2, after), 0);
    }

    #[test]
    fn payday_is_last_working_day_and_rolls_after() {
        // 20 Jun 2026 → next payday Tue 30 Jun (last working day of June).
        let today = NaiveDate::from_ymd_opt(2026, 6, 20).unwrap();
        let p = next_payday(&cfg(), today);
        assert_eq!(p, NaiveDate::from_ymd_opt(2026, 6, 30).unwrap());
        assert_eq!((p - today).num_days(), 10);
        // On payday itself it rolls to next month's last working day (Fri 31 Jul).
        let on_payday = NaiveDate::from_ymd_opt(2026, 6, 30).unwrap();
        assert_eq!(
            next_payday(&cfg(), on_payday),
            NaiveDate::from_ymd_opt(2026, 7, 31).unwrap()
        );
    }

    #[test]
    fn committed_dedups_and_windows() {
        let bills = vec![
            Bill {
                id: 1,
                name: "Energy".into(),
                expected_amount_min_cents: 11_000,
                expected_amount_max_cents: 11_700,
                currency: "GBP".into(),
                repeat_freq: "monthly".into(),
                next_expected_date: Some(
                    NaiveDate::from_ymd_opt(2026, 6, 26)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap()
                        .and_utc()
                        .timestamp(),
                ),
                last_paid_date: None,
                match_description_regex: None,
                enabled: 1,
                source_recurring_id: None,
                created_at: 0,
                updated_at: 0,
            },
            // duplicate mandate name with " (DD)" suffix — must be deduped, not double-counted.
            Bill {
                id: 2,
                name: "Energy (DD)".into(),
                expected_amount_min_cents: 11_000,
                expected_amount_max_cents: 11_700,
                currency: "GBP".into(),
                repeat_freq: "monthly".into(),
                next_expected_date: Some(
                    NaiveDate::from_ymd_opt(2026, 6, 26)
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap()
                        .and_utc()
                        .timestamp(),
                ),
                last_paid_date: None,
                match_description_regex: None,
                enabled: 1,
                source_recurring_id: None,
                created_at: 0,
                updated_at: 0,
            },
        ];
        let from = NaiveDate::from_ymd_opt(2026, 6, 20)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap()
            .and_utc()
            .timestamp();
        let payday = NaiveDate::from_ymd_opt(2026, 6, 30)
            .unwrap()
            .and_hms_opt(23, 59, 59)
            .unwrap()
            .and_utc()
            .timestamp();
        assert_eq!(committed_before_payday(&bills, from, payday), 11_700);
    }
}
