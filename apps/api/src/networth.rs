//! Daily net-worth recording — the headline numbers (cash, debt, investments,
//! pension, net) written once per day into `net_worth_history` so the dashboard can
//! chart the curve and chats can answer "am I up this month?".
//!
//! Uses the SAME classification helpers as the MCP snapshot (`snapshot_balances`,
//! `invest_values`), so a history row is exactly what get_snapshot would have said
//! that day. Recorded by the scheduler daily and at app startup; upsert by day, so
//! restarts and multiple runs are harmless (latest value for the day wins).

use crate::AppState;
use anyhow::Result;
use std::sync::Arc;

#[derive(Debug, Clone, Copy)]
pub struct Totals {
    pub cash_cents: i64,
    pub debt_cents: i64,
    pub investments_cents: i64,
    pub pension_cents: i64,
    pub net_cents: i64,
}

pub async fn compute_totals(state: &AppState) -> Result<Totals> {
    let accounts = state.db.list_all_enabled_accounts().await?;
    let plans = state.db.list_plan_accounts().await?;
    let holdings = state.db.list_holdings().await?;
    let brokers = state.db.list_brokers().await?;
    // Make sure FX rates exist so USD holdings don't get counted 1:1.
    let quotes = state.db.all_latest_quotes().await?;
    let ccys = holdings
        .iter()
        .map(|h| h.currency.clone())
        .chain(quotes.iter().map(|q| q.currency.clone()));
    crate::fx::ensure_fx_quotes(state, ccys).await;
    let quotes = state.db.all_latest_quotes().await?;

    let (debts, cash) = crate::routes::mcp_render::snapshot_balances(&accounts, &plans);
    let debt_cents: i64 = debts.iter().map(|(_, owed, _, _)| owed).sum();
    let cash_cents: i64 = cash.iter().map(|(_, c)| c).sum();
    let (invest_value, _cost, pension_value) =
        crate::routes::mcp_render::invest_values(&holdings, &quotes, &brokers);
    let investments_cents = (invest_value * 100.0).round() as i64;
    let pension_cents = (pension_value * 100.0).round() as i64;

    Ok(Totals {
        cash_cents,
        debt_cents,
        investments_cents,
        pension_cents,
        net_cents: cash_cents + investments_cents - debt_cents,
    })
}

/// Compute today's totals and upsert the row for today (UTC). Best-effort logging
/// wrapper for the scheduler/startup call sites.
pub async fn record_daily_snapshot(state: Arc<AppState>) {
    let day = chrono::Utc::now().date_naive().to_string();
    match compute_totals(&state).await {
        Ok(t) => match state.db.upsert_net_worth_snapshot(&day, &t).await {
            Ok(()) => tracing::info!(
                "net worth recorded for {day}: net £{:.2}",
                t.net_cents as f64 / 100.0
            ),
            Err(e) => tracing::warn!("net worth record failed: {e:#}"),
        },
        Err(e) => tracing::warn!("net worth compute failed: {e:#}"),
    }
}
