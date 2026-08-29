//! Retirement forecast: "I want to retire at N — am I on track, and what must I save?"
//!
//! Model (all in TODAY'S money; growth_pct is REAL growth, i.e. after inflation):
//! - The LGPS defined benefit pays `salary/49 × service years` per year from
//!   `lgps_age` (~67), inflation-linked — it cannot fund earlier retirement.
//! - The state pension pays from `state_pension_age`.
//! - The invested pots (AJ Bell SIPP + future SCAVCs, i.e. holdings under brokers of
//!   kind 'sipp') are the BRIDGE: they must cover full target income from target_age
//!   until the LGPS starts, top-ups until the state pension starts, and 25× any
//!   remaining shortfall thereafter (4% rule).
//! - Required pot is computed without growth during drawdown phases A/B, which is
//!   mildly conservative; the 25× tail handles the long run.

use crate::AppState;
use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, format!("{e}"))
}

/// User assumptions. Stored as one JSON row; unknown fields are preserved on merge.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetirementPlan {
    /// ISO date of birth — drives every "years until" number.
    pub birth_date: Option<String>,
    pub target_age: f64,
    /// Desired annual retirement income in pounds, today's money.
    pub target_income_annual: f64,
    /// REAL annual growth of invested pots (after inflation), percent.
    pub growth_pct: f64,
    /// Current gross salary in pounds — sets LGPS accrual (1/49th per service year).
    pub salary_annual: f64,
    /// ISO date LGPS membership started.
    pub lgps_start: Option<String>,
    /// Age the LGPS defined benefit starts paying (normal pension age ≈ state pension age).
    pub lgps_age: f64,
    pub state_pension_annual: f64,
    pub state_pension_age: f64,
    /// What you currently put into the invested pots per month (SIPP + SCAVCs), pounds.
    pub monthly_contribution: f64,
    /// Include non-pension (non-sipp) holdings in the starting pot.
    pub include_general_investments: bool,
}

impl Default for RetirementPlan {
    fn default() -> Self {
        Self {
            birth_date: None,
            target_age: 57.0,
            target_income_annual: 16_000.0,
            growth_pct: 5.0,
            salary_annual: 33_000.0,
            lgps_start: None,
            lgps_age: 67.0,
            state_pension_annual: 12_000.0,
            state_pension_age: 68.0,
            monthly_contribution: 0.0,
            include_general_investments: false,
        }
    }
}

fn years_between(from: chrono::NaiveDate, to: chrono::NaiveDate) -> f64 {
    (to - from).num_days() as f64 / 365.25
}

fn parse_date(s: &Option<String>) -> Option<chrono::NaiveDate> {
    s.as_deref()
        .and_then(|s| chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").ok())
}

/// Everything derived from the plan + current pension pot value.
#[derive(Debug, Serialize)]
pub struct Forecast {
    pub configured: bool,
    pub age_now: Option<f64>,
    pub years_to_target: Option<f64>,
    pub pot_now: f64,
    /// Projected LGPS annual income at target age (today's money).
    pub lgps_annual_at_target: f64,
    pub lgps_service_years_at_target: f64,
    /// Pot needed at target age to fund the plan.
    pub required_pot: f64,
    /// Phase breakdown of required_pot: bridge to LGPS, top-up to state pension, 25× tail.
    pub bridge_cost: f64,
    pub topup_cost: f64,
    pub perpetual_cost: f64,
    /// Pot you're projected to have at target age with current contributions.
    pub projected_pot: f64,
    pub on_track: bool,
    pub shortfall: f64,
    /// Monthly saving needed from now to hit required_pot exactly.
    pub required_monthly: f64,
    /// Year-by-year projection for charting: [{age, projected}].
    pub projection: Vec<Value>,
}

pub fn compute_forecast(plan: &RetirementPlan, pot_now: f64) -> Forecast {
    let today = chrono::Utc::now().date_naive();
    let birth = parse_date(&plan.birth_date);
    let age_now = birth.map(|b| years_between(b, today));
    let years_to_target = age_now.map(|a| (plan.target_age - a).max(0.0));

    // LGPS accrual: 1/49th of salary per service year, from lgps_start until target age.
    let service_years = match (birth, parse_date(&plan.lgps_start)) {
        (Some(b), Some(start)) => {
            let target_date = b + chrono::Duration::days((plan.target_age * 365.25) as i64);
            (years_between(start, target_date)).max(0.0)
        }
        _ => 0.0,
    };
    let lgps_annual = plan.salary_annual / 49.0 * service_years;

    // Required pot at target age (today's money, conservative — no growth in A/B).
    let bridge_years = (plan.lgps_age - plan.target_age).max(0.0);
    let bridge_cost = plan.target_income_annual * bridge_years;
    let topup_years = (plan.state_pension_age - plan.lgps_age).max(0.0);
    let topup_cost = (plan.target_income_annual - lgps_annual).max(0.0) * topup_years;
    let perpetual_gap =
        (plan.target_income_annual - lgps_annual - plan.state_pension_annual).max(0.0);
    let perpetual_cost = perpetual_gap * 25.0; // 4% rule
    let required_pot = bridge_cost + topup_cost + perpetual_cost;

    // Projection with current contributions, monthly compounding of real growth.
    let r_m = (1.0 + plan.growth_pct / 100.0).powf(1.0 / 12.0) - 1.0;
    let months = years_to_target.unwrap_or(0.0) * 12.0;
    let fv = |monthly: f64, m: f64| -> f64 {
        let grown = pot_now * (1.0 + r_m).powf(m);
        if r_m.abs() < 1e-12 {
            grown + monthly * m
        } else {
            grown + monthly * ((1.0 + r_m).powf(m) - 1.0) / r_m
        }
    };
    let projected_pot = fv(plan.monthly_contribution, months);
    let shortfall = (required_pot - projected_pot).max(0.0);

    // Solve the monthly contribution that lands exactly on required_pot.
    let required_monthly = if months < 1.0 {
        0.0
    } else {
        let growth_only = pot_now * (1.0 + r_m).powf(months);
        let annuity = if r_m.abs() < 1e-12 {
            months
        } else {
            ((1.0 + r_m).powf(months) - 1.0) / r_m
        };
        ((required_pot - growth_only) / annuity).max(0.0)
    };

    // Year-by-year curve for the chart.
    let mut projection = Vec::new();
    if let (Some(age), Some(yrs)) = (age_now, years_to_target) {
        let whole_years = yrs.ceil() as i64;
        for y in 0..=whole_years {
            let m = (y as f64 * 12.0).min(months);
            projection.push(json!({
                "age": (age + y as f64).round(),
                "projected": (fv(plan.monthly_contribution, m) * 100.0).round() / 100.0,
                "with_required": (fv(required_monthly, m) * 100.0).round() / 100.0,
            }));
        }
    }

    Forecast {
        configured: birth.is_some(),
        age_now,
        years_to_target,
        pot_now,
        lgps_annual_at_target: lgps_annual,
        lgps_service_years_at_target: service_years,
        required_pot,
        bridge_cost,
        topup_cost,
        perpetual_cost,
        projected_pot,
        on_track: projected_pot >= required_pot,
        shortfall,
        required_monthly,
        projection,
    }
}

/// Load the stored plan (or defaults) and the current GBP value of the pension pot.
pub async fn load_plan_and_pot(state: &AppState) -> anyhow::Result<(RetirementPlan, f64)> {
    let plan: RetirementPlan = match state.db.get_retirement_plan_json().await? {
        Some(s) => serde_json::from_str(&s).unwrap_or_default(),
        None => RetirementPlan::default(),
    };
    let holdings = state.db.list_holdings().await?;
    let brokers = state.db.list_brokers().await?;
    let sipp_brokers: std::collections::HashSet<i64> = brokers
        .iter()
        .filter(|b| b.kind == "sipp")
        .map(|b| b.id)
        .collect();
    let quotes = state.db.all_latest_quotes().await?;
    let rates = crate::fx::rates_from_quotes(&quotes);
    let by_symbol: std::collections::HashMap<_, _> =
        quotes.iter().map(|q| (q.symbol.clone(), q)).collect();
    let mut pot = 0.0;
    for h in &holdings {
        if !plan.include_general_investments && !sipp_brokers.contains(&h.broker_id) {
            continue;
        }
        if let Some(q) = by_symbol.get(&h.symbol) {
            pot += crate::fx::to_gbp(&rates, q.price * h.quantity, &q.currency);
        }
    }
    Ok((plan, pot))
}

/// Settings key for the free-form pension notes markdown shown on the Retirement
/// page and readable/writable by the MCP assistant. Same storage pattern as the
/// Ahead page's "ahead_action_plan".
pub const NOTES_KEY: &str = "pension_notes";

pub async fn get(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let (plan, pot) = load_plan_and_pot(&state).await.map_err(internal)?;
    let forecast = compute_forecast(&plan, pot);
    Ok(Json(json!({ "plan": plan, "forecast": forecast })))
}

pub async fn get_notes(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let text = state.db.get_setting(NOTES_KEY).await.map_err(internal)?.unwrap_or_default();
    Ok(Json(json!({ "text": text })))
}

#[derive(Debug, Deserialize)]
pub struct NotesBody {
    pub text: String,
}

pub async fn put_notes(
    State(state): State<Arc<AppState>>,
    Json(b): Json<NotesBody>,
) -> Result<Json<Value>, (StatusCode, String)> {
    state.db.set_setting(NOTES_KEY, &b.text).await.map_err(internal)?;
    Ok(Json(json!({ "ok": true })))
}

#[derive(Debug, Deserialize)]
pub struct HistoryQuery {
    pub days: Option<i64>,
}

/// GET /api/net-worth/history?days=365 — the recorded daily headline numbers, pounds.
pub async fn net_worth_history(
    State(state): State<Arc<AppState>>,
    axum::extract::Query(q): axum::extract::Query<HistoryQuery>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let days = q.days.unwrap_or(365).clamp(1, 3650);
    let rows = state.db.list_net_worth_history(days).await.map_err(internal)?;
    let points: Vec<Value> = rows
        .into_iter()
        .map(|(day, cash, debt, invest, pension, net)| {
            json!({
                "day": day,
                "cash": cash as f64 / 100.0,
                "debt": debt as f64 / 100.0,
                "investments": invest as f64 / 100.0,
                "pension": pension as f64 / 100.0,
                "net": net as f64 / 100.0,
            })
        })
        .collect();
    Ok(Json(json!({ "points": points })))
}

/// PUT /api/retirement — merge the provided fields into the stored plan.
pub async fn put(
    State(state): State<Arc<AppState>>,
    Json(body): Json<Value>,
) -> Result<Json<Value>, (StatusCode, String)> {
    let current: RetirementPlan = match state
        .db
        .get_retirement_plan_json()
        .await
        .map_err(internal)?
    {
        Some(s) => serde_json::from_str(&s).unwrap_or_default(),
        None => RetirementPlan::default(),
    };
    let mut merged = serde_json::to_value(&current).map_err(internal)?;
    if let (Some(obj), Some(patch)) = (merged.as_object_mut(), body.as_object()) {
        for (k, v) in patch {
            obj.insert(k.clone(), v.clone());
        }
    }
    let plan: RetirementPlan = serde_json::from_value(merged)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("invalid plan: {e}")))?;
    state
        .db
        .set_retirement_plan_json(&serde_json::to_string(&plan).map_err(internal)?)
        .await
        .map_err(internal)?;
    let (_, pot) = load_plan_and_pot(&state).await.map_err(internal)?;
    let forecast = compute_forecast(&plan, pot);
    Ok(Json(json!({ "plan": plan, "forecast": forecast })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> RetirementPlan {
        RetirementPlan {
            birth_date: Some("2000-01-01".into()),
            target_age: 57.0,
            target_income_annual: 16_000.0,
            growth_pct: 5.0,
            salary_annual: 40_000.0,
            lgps_start: Some("2025-09-01".into()),
            lgps_age: 67.0,
            state_pension_annual: 12_000.0,
            state_pension_age: 68.0,
            monthly_contribution: 200.0,
            include_general_investments: false,
        }
    }

    #[test]
    fn bridge_dominates_required_pot() {
        let f = compute_forecast(&plan(), 2_000.0);
        // 10 bridge years × £16k = £160k of the requirement.
        assert!((f.bridge_cost - 160_000.0).abs() < 1e-6);
        // ~31 service years × £40k/49 ≈ £25k LGPS ≥ target income → no tail cost.
        assert!(f.lgps_annual_at_target > 16_000.0);
        assert_eq!(f.perpetual_cost, 0.0);
        assert_eq!(f.topup_cost, 0.0);
        assert!((f.required_pot - f.bridge_cost).abs() < 1e-6);
    }

    #[test]
    fn required_monthly_hits_target_exactly() {
        let p = plan();
        let f = compute_forecast(&p, 2_000.0);
        let mut check = p.clone();
        check.monthly_contribution = f.required_monthly;
        let f2 = compute_forecast(&check, 2_000.0);
        assert!(
            (f2.projected_pot - f.required_pot).abs() < 1.0,
            "projected {} vs required {}",
            f2.projected_pot,
            f.required_pot
        );
        assert!(f2.on_track || f.required_monthly == 0.0);
    }

    #[test]
    fn unconfigured_without_birth_date() {
        let mut p = plan();
        p.birth_date = None;
        let f = compute_forecast(&p, 0.0);
        assert!(!f.configured);
        assert!(f.projection.is_empty());
    }
}
