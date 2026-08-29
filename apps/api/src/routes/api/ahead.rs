//! The "Ahead" forecast — the planning layer's read model.
//!
//! Pulls plan_accounts (synced + manual) and plan_events, expands recurring events forward over
//! a date window, turns each occurrence into per-account `legs` (a transfer is one event that
//! moves money out of one account and into another), and computes each account's projected
//! running balance + low point. Shared by the REST handler and the `list_ahead` MCP tool so the
//! frontend grid, the assistant and any future Telegram ping all agree.

use crate::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{Datelike, Duration, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct AheadQuery {
    /// How many days forward to project (default 180, clamped 7..=730).
    pub days: Option<i64>,
}

fn last_day_of_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    (NaiveDate::from_ymd_opt(ny, nm, 1).unwrap() - Duration::days(1)).day()
}

/// Add `months` calendar months to a date, clamping the day to the target month's length
/// (so the 31st becomes the 30th / 28th where needed).
fn add_months(d: NaiveDate, months: u32) -> NaiveDate {
    let total = d.month0() as i32 + months as i32;
    let year = d.year() + total.div_euclid(12);
    let month = total.rem_euclid(12) as u32 + 1;
    let day = d.day().min(last_day_of_month(year, month));
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

/// All occurrence dates of an event within [from, to], expanding its recurrence.
fn occurrences(
    start: NaiveDate,
    recurrence: &str,
    recur_until: Option<NaiveDate>,
    from: NaiveDate,
    to: NaiveDate,
) -> Vec<NaiveDate> {
    let mut out = Vec::new();
    let mut cur = start;
    for _ in 0..800 {
        if let Some(u) = recur_until {
            if cur > u {
                break;
            }
        }
        if cur > to {
            break;
        }
        if cur >= from {
            out.push(cur);
        }
        cur = match recurrence {
            "daily" => cur + Duration::days(1),
            "weekly" => cur + Duration::days(7),
            "fortnightly" => cur + Duration::days(14),
            "monthly" => add_months(cur, 1),
            "yearly" => add_months(cur, 12),
            _ => break, // "none"
        };
    }
    out
}

/// Build the full forecast object (camelCase, matching the frontend's expectations).
pub async fn build_forecast(state: &AppState, days: i64) -> anyhow::Result<Value> {
    state.db.seed_plan_accounts_from_synced().await?;
    let accounts = state.db.list_plan_accounts().await?;
    let events = state.db.list_plan_events().await?;
    let goals = state.db.list_goals().await?;
    // Plain-English "moves to make" checklist (markdown-ish), authored by the user or assistant.
    let action_plan = state.db.get_setting("ahead_action_plan").await?.unwrap_or_default();

    let today = Utc::now().date_naive();
    let from = today;
    let to = today + Duration::days(days);
    let today_iso = today.format("%Y-%m-%d").to_string();

    // Expand events → occurrences, each carrying its per-account legs.
    struct Occ {
        occ_id: String,
        date: NaiveDate,
        date_iso: String,
        label: String,
        source: String,
        recurrence: String,
        instance: bool,
        note: Option<String>,
        legs: Vec<(i64, i64)>, // (plan_account_id, delta_cents)
    }
    let mut occs: Vec<Occ> = Vec::new();
    for ev in &events {
        let Ok(start) = NaiveDate::parse_from_str(&ev.date, "%Y-%m-%d") else { continue };
        let until = ev
            .recur_until
            .as_deref()
            .and_then(|s| NaiveDate::parse_from_str(s, "%Y-%m-%d").ok());
        let legs: Vec<(i64, i64)> = match (ev.account_id, ev.to_account_id) {
            (Some(a), Some(b)) => vec![(a, -ev.amount_cents), (b, ev.amount_cents)], // transfer
            (Some(a), None) => vec![(a, ev.amount_cents)],                            // signed
            _ => vec![],                                                              // note/marker
        };
        for d in occurrences(start, &ev.recurrence, until, from, to) {
            let date_iso = d.format("%Y-%m-%d").to_string();
            occs.push(Occ {
                occ_id: format!("{}-{}", ev.id, date_iso),
                date: d,
                date_iso,
                label: ev.label.clone(),
                source: ev.source.clone(),
                recurrence: ev.recurrence.clone(),
                instance: d != start,
                note: ev.note.clone(),
                legs: legs.clone(),
            });
        }
    }
    occs.sort_by(|a, b| a.date.cmp(&b.date).then(a.occ_id.cmp(&b.occ_id)));

    // Synced accounts carry a live balance that already reflects everything up to now, so a
    // planned event dated today (or earlier) would double-count what the balance already shows
    // (e.g. salary that has already landed). Manual accounts have no live feed — their planned
    // events ARE the only signal of a movement — so they keep applying from today. So suppress a
    // leg only when its target account is synced AND the occurrence is not in the future.
    let synced_ids: std::collections::HashSet<i64> =
        accounts.iter().filter(|a| a.source == "synced").map(|a| a.id).collect();
    let leg_suppressed =
        |acc: i64, occ_date: NaiveDate| synced_ids.contains(&acc) && occ_date <= today;

    // Floor-overflow links: when an account would fall below its floor, the shortfall is drawn
    // from its overflow target instead (e.g. Nationwide floors at £0 and pushes the deficit onto
    // Santander's overdraft). One-way (cover the shortfall; never auto-repay) — repayment is an
    // explicit transfer event. NULL overflow = the floor stays a pure warning, as before.
    let overflow: Vec<(i64, i64)> =
        accounts.iter().filter_map(|a| a.overflow_account_id.map(|t| (a.id, t))).collect();
    let run_cascade = |bal: &mut HashMap<i64, i64>, date_iso: &str| {
        for (src, tgt) in &overflow {
            let Some(a) = accounts.iter().find(|a| a.id == *src) else { continue };
            let floor = a.floor_on(date_iso);
            let b = *bal.get(src).unwrap_or(&0);
            if b < floor {
                let short = floor - b;
                *bal.entry(*src).or_insert(0) += short;
                *bal.entry(*tgt).or_insert(0) -= short;
            }
        }
    };

    // Project running balances and track each account's low point across the window. The low is
    // tracked on END-OF-DAY balances (after that day's events AND the cascade), so same-day event
    // ordering can't create a phantom intra-day trough.
    let mut bal: HashMap<i64, i64> = accounts.iter().map(|a| (a.id, a.forecast_balance())).collect();
    run_cascade(&mut bal, &today_iso);
    let mut low: HashMap<i64, (i64, String)> =
        accounts.iter().map(|a| (a.id, (bal[&a.id], today_iso.clone()))).collect();
    let mut i = 0;
    while i < occs.len() {
        let date_iso = occs[i].date_iso.clone();
        while i < occs.len() && occs[i].date_iso == date_iso {
            for (acc, delta) in &occs[i].legs {
                if leg_suppressed(*acc, occs[i].date) {
                    continue; // already reflected in the live synced balance
                }
                *bal.entry(*acc).or_insert(0) += delta;
            }
            i += 1;
        }
        run_cascade(&mut bal, &date_iso);
        for a in &accounts {
            let b = bal[&a.id];
            if b < low[&a.id].0 {
                low.insert(a.id, (b, date_iso.clone()));
            }
        }
    }

    // Cliff markers (e.g. a 0% overdraft buffer ending) derived from account metadata.
    let mut markers: Vec<Value> = Vec::new();
    for a in &accounts {
        if let Some(cd) = &a.cliff_date {
            if let Ok(d) = NaiveDate::parse_from_str(cd, "%Y-%m-%d") {
                if d >= from && d <= to {
                    markers.push(json!({
                        "dateIso": cd,
                        "label": format!("{} buffer ends", a.name),
                        "accountId": a.id,
                    }));
                }
            }
        }
    }

    let accounts_json: Vec<Value> = accounts
        .iter()
        .map(|a| {
            let (low_cents, low_date) = low[&a.id].clone();
            json!({
                "id": a.id,
                "name": a.name,
                "kind": a.kind,
                "source": a.source,
                "balanceCents": a.forecast_balance(),
                "currency": a.currency,
                "floorCents": a.floor_cents,
                "overflowAccountId": a.overflow_account_id,
                "cliffDateIso": a.cliff_date,
                "cliffNewFloorCents": a.cliff_new_floor_cents,
                "creditLimitCents": a.credit_limit_cents,
                "aprBps": a.apr_bps,
                "statementDay": a.statement_day,
                "paymentIntent": a.payment_intent,
                "balanceUpdatedAt": a.balance_updated_at,
                "isManual": a.source == "manual",
                "lowCents": low_cents,
                "lowDateIso": low_date,
            })
        })
        .collect();

    let events_json: Vec<Value> = occs
        .iter()
        .map(|o| {
            // Fully reconciled = every leg already reflected in a live synced balance, so it was
            // not applied to the projection. The frontend can grey these / show a ✓ instead of a
            // pending chip, so a salary that's already arrived doesn't look like it's still due.
            let reconciled = !o.legs.is_empty() && o.legs.iter().all(|(a, _)| leg_suppressed(*a, o.date));
            json!({
                "id": o.occ_id,
                "dateIso": o.date_iso,
                "label": o.label,
                "source": o.source,
                "recurrence": o.recurrence,
                "instance": o.instance,
                "note": o.note,
                "reconciled": reconciled,
                "legs": o.legs.iter().map(|(a, d)| json!({"accountId": a, "deltaCents": d})).collect::<Vec<_>>(),
            })
        })
        .collect();

    let goals_json: Vec<Value> = goals
        .iter()
        .map(|g| {
            json!({
                "id": g.id,
                "name": g.name,
                "targetCents": g.target_cents,
                "savedCents": g.saved_cents,
                "sourceAccountId": g.source_account_id,
                "targetDateIso": g.target_date,
                "monthlyCents": g.monthly_cents,
            })
        })
        .collect();

    // Past balance history (left of "today" on the graph): daily snapshots grouped by day.
    let hist_from = (today - Duration::days(30)).format("%Y-%m-%d").to_string();
    let snaps = state.db.list_balance_snapshots_since(&hist_from).await.unwrap_or_default();
    let mut by_day: std::collections::BTreeMap<String, Vec<(i64, i64)>> = std::collections::BTreeMap::new();
    for (acc, day, bal) in snaps {
        by_day.entry(day).or_default().push((acc, bal));
    }
    let history_json: Vec<Value> = by_day
        .into_iter()
        .map(|(day, balances)| {
            json!({
                "dateIso": day,
                "balances": balances.iter().map(|(a, b)| json!({"accountId": a, "balanceCents": b})).collect::<Vec<_>>(),
            })
        })
        .collect();

    Ok(json!({
        "fromIso": today_iso,
        "toIso": to.format("%Y-%m-%d").to_string(),
        "accounts": accounts_json,
        "events": events_json,
        "markers": markers,
        "goals": goals_json,
        "history": history_json,
        "actionPlan": action_plan,
    }))
}

pub async fn get_ahead(
    State(state): State<Arc<AppState>>,
    Query(q): Query<AheadQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let days = q.days.unwrap_or(180).clamp(7, 730);
    let v = build_forecast(&state, days)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(v))
}
