//! Rendering layer for the MCP server: converts tally data into the compact, LLM-friendly
//! markdown that AI clients actually read.
//!
//! Design rules (all evidence-driven — LLMs misread raw finance JSON):
//!   * every amount is a pre-formatted GBP string with comma grouping ("£1,234.56") — never
//!     bare pence integers, never unit-ambiguous floats
//!   * debts and money owed render as POSITIVE amounts labelled "owed"/"overdrawn" — models
//!     drop negative signs, so the sign lives in words
//!   * every date is ISO (YYYY-MM-DD) — never unix seconds
//!   * every list leads with a one-line summary and pre-computed totals so the model quotes
//!     numbers instead of doing arithmetic
//!   * a shared conventions footer travels with the data (claude.ai drops the MCP server
//!     `instructions` field, so payload text is the only reliable channel)

use crate::models::{Account, Bill, Broker, Goal, Holding, LatestQuote, PlanAccount, Reminder, Transaction};
use serde_json::Value;
use std::collections::HashMap;

/// Appended to every read-tool result. Descriptions get skimmed; this travels with the data.
pub(crate) const CONVENTIONS: &str = "_All amounts are GBP. Debts are shown as positive amounts \
    marked \"owed\" or \"overdrawn\". Totals are precomputed by the server — quote them, do not \
    recompute._";

// ---------- money / date formatting ----------

fn thousands(mut n: i64) -> String {
    let neg = n < 0;
    n = n.abs();
    let mut parts = Vec::new();
    loop {
        if n < 1000 {
            parts.push(n.to_string());
            break;
        }
        parts.push(format!("{:03}", n % 1000));
        n /= 1000;
    }
    parts.reverse();
    let s = parts.join(",");
    if neg { format!("-{s}") } else { s }
}

/// "£1,234.56" (sign preserved: "-£1,234.56"). Prefer the worded helpers below for debts.
pub(crate) fn gbp(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.abs();
    format!("{sign}£{}.{:02}", thousands(abs / 100), abs % 100)
}

/// Format a float amount already in pounds (investments path).
pub(crate) fn gbp_f(pounds: f64) -> String {
    gbp((pounds * 100.0).round() as i64)
}

/// Balance for display: positive stays "£x", negative becomes "overdrawn £x".
fn balance_words(cents: i64) -> String {
    if cents < 0 {
        format!("overdrawn {}", gbp(-cents))
    } else {
        gbp(cents)
    }
}

/// Amount owed, always positive with the word carrying the sign.
fn owed_words(owed_cents: i64) -> String {
    format!("owed {}", gbp(owed_cents.abs()))
}

pub(crate) fn iso_date(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.date_naive().to_string())
        .unwrap_or_default()
}

fn iso_datetime(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

fn opt(s: &Option<String>) -> String {
    s.clone().unwrap_or_else(|| "—".into())
}

fn apr_words(apr_bps: Option<i64>) -> String {
    match apr_bps {
        Some(bps) => format!("{:.1}%", bps as f64 / 100.0),
        None => "—".into(),
    }
}

/// User-facing account label: custom name, else bank nickname, else provider name.
pub(crate) fn account_name(a: &Account) -> String {
    a.custom_display_name
        .clone()
        .or_else(|| a.consent_nickname.clone())
        .unwrap_or_else(|| a.display_name.clone())
}

// ---------- markdown ----------

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

pub(crate) fn md_table<S: AsRef<str>>(headers: &[&str], rows: &[Vec<S>]) -> String {
    let mut out = String::new();
    out.push_str("| ");
    out.push_str(&headers.join(" | "));
    out.push_str(" |\n|");
    for _ in headers {
        out.push_str("---|");
    }
    out.push('\n');
    for r in rows {
        out.push_str("| ");
        let cells: Vec<String> = r.iter().map(|c| cell(c.as_ref())).collect();
        out.push_str(&cells.join(" | "));
        out.push_str(" |\n");
    }
    out
}

// ---------- per-tool renderers ----------

pub(crate) fn render_accounts(accounts: &[Account]) -> String {
    let mut current_rows: Vec<Vec<String>> = Vec::new();
    let mut card_rows: Vec<Vec<String>> = Vec::new();
    let mut cash_total = 0i64;
    let mut card_owed_total = 0i64;

    for a in accounts {
        let bal = a.current_balance_cents.unwrap_or(0);
        let updated = a.balance_updated_at.map(iso_date).unwrap_or_else(|| "—".into());
        if a.kind == "card" {
            // Cards report what you owe as a positive number.
            card_owed_total += bal.max(0);
            let owed = if bal >= 0 { owed_words(bal) } else { format!("in credit {}", gbp(-bal)) };
            let due = match (a.payment_due_cents, &a.payment_due_date) {
                (Some(c), Some(d)) => format!("{} by {}", gbp(c), d),
                (Some(c), None) => gbp(c),
                _ => "—".into(),
            };
            card_rows.push(vec![
                account_name(a),
                owed,
                a.credit_limit_cents.map(gbp).unwrap_or_else(|| "—".into()),
                due,
                updated,
            ]);
        } else {
            cash_total += bal;
            current_rows.push(vec![
                account_name(a),
                balance_words(bal),
                a.available_balance_cents.map(gbp).unwrap_or_else(|| "—".into()),
                a.overdraft_cents.map(gbp).unwrap_or_else(|| "—".into()),
                updated,
            ]);
        }
    }

    let mut out = format!(
        "{} linked accounts. Bank balance total: {} · credit card debt total: {}.\n\n",
        accounts.len(),
        balance_words(cash_total),
        gbp(card_owed_total),
    );
    if !current_rows.is_empty() {
        out.push_str("### Current accounts\n");
        out.push_str(&md_table(
            &["Account", "Balance", "Available", "Overdraft limit", "Updated"],
            &current_rows,
        ));
        out.push('\n');
    }
    if !card_rows.is_empty() {
        out.push_str("### Credit cards\n");
        out.push_str(&md_table(
            &["Card", "Owed", "Credit limit", "Payment due", "Updated"],
            &card_rows,
        ));
        out.push('\n');
    }
    out.push_str(CONVENTIONS);
    out
}

pub(crate) fn render_transactions(
    txns: &[Transaction],
    account_names: &HashMap<i64, String>,
    category_names: &HashMap<i64, String>,
    days: i64,
    limit: i64,
) -> String {
    let total_in: i64 = txns.iter().filter(|t| t.is_credit == 1).map(|t| t.amount_cents).sum();
    let total_out: i64 = txns.iter().filter(|t| t.is_credit == 0).map(|t| t.amount_cents).sum();
    let net = total_in - total_out;
    let truncated = txns.len() as i64 >= limit;

    let rows: Vec<Vec<String>> = txns
        .iter()
        .map(|t| {
            let mut desc = t.description.clone();
            if t.is_pending == 1 {
                desc.push_str(" (pending)");
            }
            vec![
                iso_date(t.timestamp),
                desc,
                if t.is_credit == 1 { "in".into() } else { "out".into() },
                gbp(t.amount_cents),
                t.category_id
                    .and_then(|id| category_names.get(&id).cloned())
                    .unwrap_or_else(|| "—".into()),
                account_names.get(&t.account_id).cloned().unwrap_or_else(|| "—".into()),
            ]
        })
        .collect();

    let mut out = format!(
        "{} transactions from the last {} days{}. Money in: {} · money out: {} · net: {}{}.\n\n",
        txns.len(),
        days,
        if truncated {
            format!(" (hit the {limit}-row limit — older rows exist; raise `limit` or use `query`)")
        } else {
            String::new()
        },
        gbp(total_in),
        gbp(total_out),
        if net < 0 { "out " } else { "in " },
        gbp(net.abs()),
    );
    out.push_str(&md_table(
        &["Date", "Description", "In/Out", "Amount", "Category", "Account"],
        &rows,
    ));
    out.push('\n');
    out.push_str(CONVENTIONS);
    out
}

pub(crate) fn render_spending(
    rows: &[(Option<i64>, i64)],
    category_names: &HashMap<i64, String>,
    days: i64,
) -> String {
    let total: i64 = rows.iter().map(|(_, c)| *c).sum();
    let mut sorted: Vec<&(Option<i64>, i64)> = rows.iter().collect();
    sorted.sort_by_key(|(_, c)| -c);
    let table: Vec<Vec<String>> = sorted
        .iter()
        .map(|(cat, cents)| {
            vec![
                cat.and_then(|id| category_names.get(&id).cloned())
                    .unwrap_or_else(|| "Uncategorised".into()),
                gbp(*cents),
            ]
        })
        .collect();
    format!(
        "Total spent over the last {days} days: {}.\n\n{}\n{}",
        gbp(total),
        md_table(&["Category", "Spent"], &table),
        CONVENTIONS
    )
}

pub(crate) fn render_bills(bills: &[Bill], within_days: Option<i64>) -> String {
    let now = chrono::Utc::now().timestamp();
    let expected = |b: &Bill| -> i64 {
        if b.expected_amount_max_cents > 0 { b.expected_amount_max_cents } else { b.expected_amount_min_cents }
    };
    let total: i64 = bills.iter().map(expected).sum();
    let rows: Vec<Vec<String>> = bills
        .iter()
        .map(|b| {
            let amount = if b.expected_amount_max_cents > b.expected_amount_min_cents {
                format!("{}–{}", gbp(b.expected_amount_min_cents), gbp(b.expected_amount_max_cents))
            } else {
                gbp(expected(b))
            };
            let due = match b.next_expected_date {
                Some(ts) => {
                    let days = (ts - now) / 86_400;
                    if days >= 0 { format!("{} (in {} days)", iso_date(ts), days) } else { iso_date(ts) }
                }
                None => "—".into(),
            };
            vec![b.name.clone(), amount, b.repeat_freq.clone(), due]
        })
        .collect();
    format!(
        "{} recurring bills{}. Expected total (upper bound): {}.\n\n{}\n{}",
        bills.len(),
        within_days.map(|d| format!(" due within {d} days")).unwrap_or_default(),
        gbp(total),
        md_table(&["Bill", "Expected", "Frequency", "Next due"], &rows),
        CONVENTIONS
    )
}

/// Markdown for the `retirement_forecast` MCP tool.
pub(crate) fn render_retirement(
    plan: &crate::routes::api::retirement::RetirementPlan,
    f: &crate::routes::api::retirement::Forecast,
) -> String {
    if !f.configured {
        return format!(
            "Retirement forecast is not configured yet — set birth_date (and ideally \
             lgps_start, salary_annual, target_age) with set_retirement_plan or on the \
             Retirement page.\n\n{CONVENTIONS}"
        );
    }
    let verdict = if f.on_track {
        format!(
            "ON TRACK — projected pot {} vs {} required.",
            gbp_f(f.projected_pot),
            gbp_f(f.required_pot)
        )
    } else {
        format!(
            "NOT on track yet: projected pot {} vs {} required — shortfall {}. Saving {} /month (instead of the current {}) would close it.",
            gbp_f(f.projected_pot),
            gbp_f(f.required_pot),
            gbp_f(f.shortfall),
            gbp_f(f.required_monthly),
            gbp_f(plan.monthly_contribution),
        )
    };
    format!(
        "# Retirement at {} — {}\n\n\
         - Age now: {:.1}; {:.1} years to go. Pension pot today: {}.\n\
         - LGPS defined benefit at {}: ~{} /year from age {} ({:.1} service years × salary {} / 49, today's money).\n\
         - State pension: {} /year from age {}.\n\
         - Required pot at {}: {} = bridge {} (full income {}→{}) + top-up {} ({}→{}) + {} (25× any remaining gap).\n\
         - Assumptions: target income {} /year (today's money), real growth {}%/year, current contributions {} /month.\n\n{}",
        plan.target_age,
        verdict,
        f.age_now.unwrap_or(0.0),
        f.years_to_target.unwrap_or(0.0),
        gbp_f(f.pot_now),
        plan.target_age,
        gbp_f(f.lgps_annual_at_target),
        plan.lgps_age,
        f.lgps_service_years_at_target,
        gbp_f(plan.salary_annual),
        gbp_f(plan.state_pension_annual),
        plan.state_pension_age,
        plan.target_age,
        gbp_f(f.required_pot),
        gbp_f(f.bridge_cost),
        plan.target_age,
        plan.lgps_age,
        gbp_f(f.topup_cost),
        plan.lgps_age,
        plan.state_pension_age,
        gbp_f(f.perpetual_cost),
        gbp_f(plan.target_income_annual),
        plan.growth_pct,
        gbp_f(plan.monthly_contribution),
        CONVENTIONS
    )
}

pub(crate) fn render_investments(
    holdings: &[Holding],
    quotes: &[LatestQuote],
    brokers: &[crate::models::Broker],
) -> String {
    let rates = crate::fx::rates_from_quotes(quotes);
    let by_symbol: HashMap<&str, &LatestQuote> =
        quotes.iter().map(|q| (q.symbol.as_str(), q)).collect();
    let broker_by_id: HashMap<i64, &crate::models::Broker> =
        brokers.iter().map(|b| (b.id, b)).collect();
    let mut total = 0.0f64;
    let mut cost = 0.0f64;
    let mut pension_total = 0.0f64;
    let mut rows: Vec<Vec<String>> = Vec::new();
    for h in holdings {
        let broker = broker_by_id.get(&h.broker_id);
        let is_pension = broker.map(|b| b.kind == "sipp").unwrap_or(false);
        let quote = by_symbol.get(h.symbol.as_str());
        // Values/gains in GBP via cached FX; the per-unit price stays in the
        // instrument's native currency so it matches what the broker shows.
        let value = quote.map(|q| crate::fx::to_gbp(&rates, q.price * h.quantity, &q.currency));
        let hold_cost = h
            .avg_cost_per_unit
            .map(|c| crate::fx::to_gbp(&rates, c * h.quantity, &h.currency));
        total += value.unwrap_or(0.0);
        cost += hold_cost.unwrap_or(0.0);
        if is_pension {
            pension_total += value.unwrap_or(0.0);
        }
        let gain = match (value, hold_cost) {
            (Some(v), Some(c)) => {
                let g = v - c;
                if g >= 0.0 { format!("up {}", gbp_f(g)) } else { format!("down {}", gbp_f(-g)) }
            }
            _ => "—".into(),
        };
        rows.push(vec![
            h.symbol.clone(),
            h.name.clone().unwrap_or_else(|| h.symbol.clone()),
            broker
                .map(|b| {
                    if b.kind == "sipp" {
                        format!("{} (pension)", b.name)
                    } else {
                        b.name.clone()
                    }
                })
                .unwrap_or_else(|| "—".into()),
            format!("{}", h.quantity),
            quote
                .map(|q| {
                    if q.currency == "GBP" {
                        gbp_f(q.price)
                    } else {
                        format!("{:.2} {}", q.price, q.currency)
                    }
                })
                .unwrap_or_else(|| "no quote".into()),
            value.map(gbp_f).unwrap_or_else(|| "—".into()),
            gain,
        ]);
    }
    let gain = total - cost;
    let pct = if cost > 0.0 { format!(" ({:+.1}%)", gain / cost * 100.0) } else { String::new() };
    let split = if pension_total > 0.0 && pension_total < total {
        format!(
            " Of that, {} is pension (SIPP) and {} is regular investments.",
            gbp_f(pension_total),
            gbp_f(total - pension_total)
        )
    } else if pension_total > 0.0 {
        " All of it is pension (SIPP).".to_string()
    } else {
        String::new()
    };
    format!(
        "Investments: {} holdings worth {} (cost basis {}, unrealised {}{}).{}\n\n{}\n{}",
        holdings.len(),
        gbp_f(total),
        gbp_f(cost),
        if gain >= 0.0 { format!("gain {}", gbp_f(gain)) } else { format!("loss {}", gbp_f(-gain)) },
        pct,
        split,
        md_table(&["Symbol", "Name", "Broker", "Qty", "Last price", "Value", "Gain"], &rows),
        CONVENTIONS
    )
}

/// Render the JSON produced by `safe_to_spend::compute` (camelCase cents fields).
pub(crate) fn render_safe_to_spend(v: &Value) -> String {
    let get = |k: &str| v.get(k).and_then(Value::as_i64).unwrap_or(0);
    if !v.get("configured").and_then(Value::as_bool).unwrap_or(false) {
        return format!(
            "Safe-to-spend is not configured yet (no payday/floors set up in tally), so there is \
             no runway number. Use list_accounts for balances instead.\n\n{CONVENTIONS}"
        );
    }
    let payday = iso_date(get("nextPaydayUnix"));
    let per_account: Vec<Vec<String>> = v
        .get("perAccount")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .map(|a| {
                    let g = |k: &str| a.get(k).and_then(Value::as_i64).unwrap_or(0);
                    vec![
                        a.get("name").and_then(Value::as_str).unwrap_or("—").to_string(),
                        balance_words(g("currentCents")),
                        gbp(g("floorCents")),
                        gbp(g("spendableCents")),
                    ]
                })
                .collect()
        })
        .unwrap_or_default();
    format!(
        "Safe to spend TODAY: {} ({} per day for the {} days until payday {}).\n\
         Spendable above floors now: {} · committed bills before payday: {} · ring-fenced: {} · \
         already spent today: {}.\n\n{}\n{}",
        gbp(get("safeTodayCents")),
        gbp(get("safePerDayCents")),
        get("daysLeft"),
        payday,
        gbp(get("spendableNowCents")),
        gbp(get("committedCents")),
        gbp(get("ringfenceCents")),
        gbp(get("spentTodayCents")),
        md_table(&["Account", "Balance", "Floor", "Spendable"], &per_account),
        CONVENTIONS
    )
}

/// Render the JSON produced by `ahead::build_forecast`. The raw payload is chart-oriented
/// (pence, account-id legs, daily history) — reshape it into tables and drop the history.
pub(crate) fn render_ahead(v: &Value) -> String {
    let empty = Vec::new();
    let accounts = v.get("accounts").and_then(Value::as_array).unwrap_or(&empty);
    let events = v.get("events").and_then(Value::as_array).unwrap_or(&empty);
    let markers = v.get("markers").and_then(Value::as_array).unwrap_or(&empty);
    let goals = v.get("goals").and_then(Value::as_array).unwrap_or(&empty);

    let names: HashMap<i64, String> = accounts
        .iter()
        .filter_map(|a| {
            Some((a.get("id")?.as_i64()?, a.get("name")?.as_str()?.to_string()))
        })
        .collect();

    let acct_rows: Vec<Vec<String>> = accounts
        .iter()
        .map(|a| {
            let g = |k: &str| a.get(k).and_then(Value::as_i64).unwrap_or(0);
            let s = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("—").to_string();
            let low = format!("{} on {}", balance_words(g("lowCents")), s("lowDateIso"));
            vec![
                s("name"),
                s("kind"),
                balance_words(g("balanceCents")),
                gbp(g("floorCents")),
                low,
            ]
        })
        .collect();

    const EVENT_CAP: usize = 60;
    let ev_rows: Vec<Vec<String>> = events
        .iter()
        .take(EVENT_CAP)
        .map(|e| {
            let s = |k: &str| e.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            let legs = e.get("legs").and_then(Value::as_array).cloned().unwrap_or_default();
            let leg_str = |l: &Value| -> (String, i64) {
                let id = l.get("accountId").and_then(Value::as_i64).unwrap_or(0);
                let d = l.get("deltaCents").and_then(Value::as_i64).unwrap_or(0);
                (names.get(&id).cloned().unwrap_or_else(|| format!("account {id}")), d)
            };
            let (amount, movement) = match legs.len() {
                2 => {
                    let (n1, d1) = leg_str(&legs[0]);
                    let (n2, _) = leg_str(&legs[1]);
                    let (from, to) = if d1 < 0 { (n1, n2) } else { (n2, n1) };
                    (gbp(d1.abs()), format!("{from} → {to}"))
                }
                1 => {
                    let (n, d) = leg_str(&legs[0]);
                    (gbp(d.abs()), if d < 0 { format!("out of {n}") } else { format!("into {n}") })
                }
                _ => ("—".into(), "—".into()),
            };
            let mut label = s("label");
            if e.get("reconciled").and_then(Value::as_bool).unwrap_or(false) {
                label.push_str(" ✓already happened");
            }
            let rec = s("recurrence");
            vec![
                s("dateIso"),
                label,
                amount,
                movement,
                if rec.is_empty() || rec == "none" { "one-off".into() } else { rec },
            ]
        })
        .collect();

    let mut out = format!(
        "Cashflow forecast {} → {}. {} planning accounts, {} upcoming events.\n\n### Accounts (with projected low point)\n{}\n",
        v.get("fromIso").and_then(Value::as_str).unwrap_or("?"),
        v.get("toIso").and_then(Value::as_str).unwrap_or("?"),
        accounts.len(),
        events.len(),
        md_table(&["Account", "Kind", "Balance now", "Floor", "Projected low"], &acct_rows),
    );
    if !ev_rows.is_empty() {
        out.push_str("\n### Upcoming events\n");
        out.push_str(&md_table(&["Date", "Event", "Amount", "Movement", "Repeats"], &ev_rows));
        if events.len() > EVENT_CAP {
            out.push_str(&format!("…plus {} more events (narrow `days` to see fewer).\n", events.len() - EVENT_CAP));
        }
    }
    for m in markers {
        out.push_str(&format!(
            "\n⚠ {}: {}",
            m.get("dateIso").and_then(Value::as_str).unwrap_or("?"),
            m.get("label").and_then(Value::as_str).unwrap_or("?"),
        ));
    }
    if !goals.is_empty() {
        let rows: Vec<Vec<String>> = goals
            .iter()
            .map(|g| {
                let gi = |k: &str| g.get(k).and_then(Value::as_i64).unwrap_or(0);
                vec![
                    g.get("name").and_then(Value::as_str).unwrap_or("—").to_string(),
                    gbp(gi("targetCents")),
                    gbp(gi("savedCents")),
                    gbp(gi("monthlyCents")),
                ]
            })
            .collect();
        out.push_str("\n\n### Goals\n");
        out.push_str(&md_table(&["Goal", "Target", "Saved", "Monthly"], &rows));
    }
    if let Some(plan) = v.get("actionPlan").and_then(Value::as_str) {
        if !plan.trim().is_empty() {
            out.push_str("\n### Current action plan (\"Moves to make\")\n");
            out.push_str(plan.trim());
            out.push('\n');
        }
    }
    out.push('\n');
    out.push_str(CONVENTIONS);
    out
}

pub(crate) fn render_plan_accounts(plans: &[PlanAccount]) -> String {
    let rows: Vec<Vec<String>> = plans
        .iter()
        .map(|p| {
            let fb = p.forecast_balance();
            let bal = if fb < 0 { owed_words(fb) } else { gbp(fb) };
            let cliff = match (&p.cliff_date, p.cliff_new_floor_cents) {
                (Some(d), Some(f)) => format!("floor → {} on {d}", gbp(f)),
                _ => "—".into(),
            };
            vec![
                p.id.to_string(),
                p.name.clone(),
                p.kind.clone(),
                p.source.clone(),
                bal,
                gbp(p.floor_cents),
                p.credit_limit_cents.map(gbp).unwrap_or_else(|| "—".into()),
                apr_words(p.apr_bps),
                cliff,
            ]
        })
        .collect();
    format!(
        "{} planning accounts (pass `id` or exact name to upsert_plan_event / set_account_balance).\n\n{}\n{}",
        plans.len(),
        md_table(
            &["id", "Name", "Kind", "Source", "Balance", "Floor", "Limit", "APR", "Cliff"],
            &rows
        ),
        CONVENTIONS
    )
}

pub(crate) fn render_goals(goals: &[Goal]) -> String {
    let target: i64 = goals.iter().map(|g| g.target_cents).sum();
    let saved: i64 = goals.iter().map(|g| g.saved_cents).sum();
    let rows: Vec<Vec<String>> = goals
        .iter()
        .map(|g| {
            let pct = if g.target_cents > 0 {
                format!("{}%", g.saved_cents * 100 / g.target_cents)
            } else {
                "—".into()
            };
            vec![
                g.id.to_string(),
                g.name.clone(),
                gbp(g.target_cents),
                gbp(g.saved_cents),
                pct,
                gbp(g.monthly_cents),
                opt(&g.target_date),
            ]
        })
        .collect();
    format!(
        "{} savings goals: {} saved of {} total.\n\n{}\n{}",
        goals.len(),
        gbp(saved),
        gbp(target),
        md_table(&["id", "Goal", "Target", "Saved", "%", "Monthly", "Target date"], &rows),
        CONVENTIONS
    )
}

pub(crate) fn render_reminders(reminders: &[Reminder]) -> String {
    let rows: Vec<Vec<String>> = reminders
        .iter()
        .filter(|r| r.archived == 0)
        .map(|r| {
            let every = if r.every_n == 1 {
                format!("every {}", r.freq.trim_end_matches('s'))
            } else {
                format!("every {} {}", r.every_n, r.freq)
            };
            vec![
                r.id.to_string(),
                r.title.clone(),
                iso_datetime(r.due_at),
                every,
                r.completed_at.map(iso_date).unwrap_or_else(|| "not yet".into()),
                opt(&r.notes),
            ]
        })
        .collect();
    format!(
        "{} active reminders (tick with tick_reminder + id).\n\n{}",
        rows.len(),
        md_table(&["id", "Reminder", "Next due", "Repeats", "Last done", "Notes"], &rows)
    )
}

pub(crate) fn render_brokers(brokers: &[Broker]) -> String {
    let rows: Vec<Vec<String>> = brokers
        .iter()
        .map(|b| vec![b.id.to_string(), b.name.clone(), b.kind.clone()])
        .collect();
    format!("{} brokers.\n\n{}", brokers.len(), md_table(&["id", "Name", "Kind"], &rows))
}

// ---------- the snapshot ----------

#[allow(clippy::too_many_arguments)]
/// The canonical debt/cash classification used by the MCP snapshot AND the daily
/// net-worth recorder — one source of truth so history rows always match what
/// get_snapshot reports. Returns (debts, cash) as
/// (name, owed_cents, limit_cents, apr_bps) and (name, cents).
pub(crate) fn snapshot_balances(
    accounts: &[Account],
    plans: &[PlanAccount],
) -> (
    Vec<(String, i64, Option<i64>, Option<i64>)>,
    Vec<(String, i64)>,
) {
    let mut debts: Vec<(String, i64, Option<i64>, Option<i64>)> = Vec::new();
    let mut cash: Vec<(String, i64)> = Vec::new();

    // Planning metadata (APR, limit) for synced accounts, keyed by the live account id.
    let plan_by_linked: HashMap<i64, &PlanAccount> =
        plans.iter().filter_map(|p| p.linked_account_id.map(|id| (id, p))).collect();

    for a in accounts {
        let bal = a.current_balance_cents.unwrap_or(0);
        let name = account_name(a);
        let meta = plan_by_linked.get(&a.id);
        if a.kind == "card" {
            if bal > 0 {
                debts.push((
                    format!("{name} (credit card)"),
                    bal,
                    a.credit_limit_cents.or(meta.and_then(|p| p.credit_limit_cents)),
                    meta.and_then(|p| p.apr_bps),
                ));
            } else if bal < 0 {
                cash.push((format!("{name} (card in credit)"), -bal));
            }
        } else if bal < 0 {
            debts.push((
                format!("{name} (overdrawn current account)"),
                -bal,
                a.overdraft_cents,
                meta.and_then(|p| p.apr_bps),
            ));
        } else {
            cash.push((name, bal));
        }
    }

    // Manual planning accounts (e.g. a credit card Open Banking can't reach). Synced plan
    // accounts mirror live accounts already counted above, so only manual ones add here.
    for p in plans.iter().filter(|p| p.source == "manual") {
        let fb = p.forecast_balance();
        if fb < 0 {
            debts.push((format!("{} (manual {})", p.name, p.kind), -fb, p.credit_limit_cents, p.apr_bps));
        } else if fb > 0 {
            cash.push((format!("{} (manual)", p.name), fb));
        }
    }

    (debts, cash)
}

/// GBP investment totals (market value, cost basis, pension subset) — shared by the
/// snapshot renderer and the net-worth recorder.
pub(crate) fn invest_values(
    holdings: &[Holding],
    quotes: &[LatestQuote],
    brokers: &[crate::models::Broker],
) -> (f64, f64, f64) {
    let rates = crate::fx::rates_from_quotes(quotes);
    let by_symbol: HashMap<&str, &LatestQuote> =
        quotes.iter().map(|q| (q.symbol.as_str(), q)).collect();
    let sipp_ids: std::collections::HashSet<i64> = brokers
        .iter()
        .filter(|b| b.kind == "sipp")
        .map(|b| b.id)
        .collect();
    let mut invest_value = 0.0f64;
    let mut invest_cost = 0.0f64;
    let mut pension_value = 0.0f64;
    for h in holdings {
        if let Some(q) = by_symbol.get(h.symbol.as_str()) {
            let v = crate::fx::to_gbp(&rates, q.price * h.quantity, &q.currency);
            invest_value += v;
            if sipp_ids.contains(&h.broker_id) {
                pension_value += v;
            }
        }
        if let Some(c) = h.avg_cost_per_unit {
            invest_cost += crate::fx::to_gbp(&rates, c * h.quantity, &h.currency);
        }
    }
    (invest_value, invest_cost, pension_value)
}

pub(crate) fn render_snapshot(
    today_iso: &str,
    accounts: &[Account],
    plans: &[PlanAccount],
    holdings: &[Holding],
    quotes: &[LatestQuote],
    brokers: &[crate::models::Broker],
    safe_to_spend: &Value,
    bills_due: &[Bill],
    goals: &[Goal],
) -> String {
    let (debts, cash) = snapshot_balances(accounts, plans);
    let debt_total: i64 = debts.iter().map(|(_, owed, _, _)| owed).sum();
    let cash_total: i64 = cash.iter().map(|(_, c)| c).sum();
    let (invest_value, invest_cost, pension_value) = invest_values(holdings, quotes, brokers);
    let invest_cents = (invest_value * 100.0).round() as i64;

    let assets_total = cash_total + invest_cents;
    let net = assets_total - debt_total;

    let mut out = format!("# Financial snapshot — {today_iso}\n\n");
    out.push_str(&format!(
        "**Net position: {}{}** = assets {} (cash {} + investments {}) minus debt {}.\n\n",
        if net < 0 { "-" } else { "" },
        gbp(net.abs()),
        gbp(assets_total),
        gbp(cash_total),
        gbp(invest_cents),
        gbp(debt_total),
    ));

    out.push_str(&format!("## Total debt: {}\n", gbp(debt_total)));
    if debts.is_empty() {
        out.push_str("No debt right now.\n");
    } else {
        let rows: Vec<Vec<String>> = debts
            .iter()
            .map(|(name, owed, limit, apr)| {
                vec![
                    name.clone(),
                    gbp(*owed),
                    limit.map(gbp).unwrap_or_else(|| "—".into()),
                    apr_words(*apr),
                ]
            })
            .collect();
        out.push_str(&md_table(&["Debt", "Owed", "Limit", "APR"], &rows));
    }

    out.push_str(&format!("\n## Cash & savings: {}\n", gbp(cash_total)));
    let cash_rows: Vec<Vec<String>> =
        cash.iter().map(|(n, c)| vec![n.clone(), gbp(*c)]).collect();
    out.push_str(&md_table(&["Account", "Balance"], &cash_rows));

    if !holdings.is_empty() {
        let gain = invest_value - invest_cost;
        let pension_note = if pension_value > 0.0 {
            format!(
                " Of that, {} is pension (SIPP — locked until ~57).",
                gbp_f(pension_value)
            )
        } else {
            String::new()
        };
        out.push_str(&format!(
            "\n## Investments: {}\n{} holdings, cost basis {}, unrealised {}.{} Details: investments_summary.\n",
            gbp_f(invest_value),
            holdings.len(),
            gbp_f(invest_cost),
            if gain >= 0.0 { format!("gain {}", gbp_f(gain)) } else { format!("loss {}", gbp_f(-gain)) },
            pension_note,
        ));
    }

    // Safe to spend headline.
    if safe_to_spend.get("configured").and_then(Value::as_bool).unwrap_or(false) {
        let g = |k: &str| safe_to_spend.get(k).and_then(Value::as_i64).unwrap_or(0);
        out.push_str(&format!(
            "\n## Safe to spend\nToday: {} ({}/day, {} days until payday {}). Committed bills before payday: {}.\n",
            gbp(g("safeTodayCents")),
            gbp(g("safePerDayCents")),
            g("daysLeft"),
            iso_date(g("nextPaydayUnix")),
            gbp(g("committedCents")),
        ));
    }

    if !bills_due.is_empty() {
        let expected = |b: &Bill| -> i64 {
            if b.expected_amount_max_cents > 0 { b.expected_amount_max_cents } else { b.expected_amount_min_cents }
        };
        let total: i64 = bills_due.iter().map(expected).sum();
        let rows: Vec<Vec<String>> = bills_due
            .iter()
            .map(|b| {
                vec![
                    b.name.clone(),
                    gbp(expected(b)),
                    b.next_expected_date.map(iso_date).unwrap_or_else(|| "—".into()),
                ]
            })
            .collect();
        out.push_str(&format!("\n## Bills due in the next 14 days: {}\n", gbp(total)));
        out.push_str(&md_table(&["Bill", "Expected", "Due"], &rows));
    }

    if !goals.is_empty() {
        let target: i64 = goals.iter().map(|g| g.target_cents).sum();
        let saved: i64 = goals.iter().map(|g| g.saved_cents).sum();
        out.push_str(&format!(
            "\n## Goals\n{} saved of {} across {} goals. Details: list_goals.\n",
            gbp(saved),
            gbp(target),
            goals.len(),
        ));
    }

    out.push('\n');
    out.push_str(CONVENTIONS);
    out
}

// ---------- query tool: validation ----------

/// Blank out single-quoted SQL string literals (handles '' escapes) so keyword checks don't
/// false-positive on words inside user-searchable text.
fn strip_string_literals(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            out.push(' ');
            while let Some(c2) = chars.next() {
                if c2 == '\'' {
                    if chars.peek() == Some(&'\'') {
                        chars.next();
                        continue;
                    }
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Validate that `sql` is a single read-only SELECT. Returns the trimmed statement.
pub(crate) fn validate_query(sql: &str) -> Result<String, String> {
    let s = sql.trim();
    let s = s.strip_suffix(';').unwrap_or(s).trim();
    if s.is_empty() {
        return Err("empty query".into());
    }
    let stripped = strip_string_literals(s);
    if stripped.contains("--") || stripped.contains("/*") {
        return Err("comments are not allowed in queries".into());
    }
    if stripped.contains(';') {
        return Err("only a single SELECT statement is allowed".into());
    }
    let lower = stripped.to_ascii_lowercase();
    if !(lower.starts_with("select") || lower.starts_with("with")) {
        return Err("only SELECT (or WITH … SELECT) queries are allowed".into());
    }
    const BANNED: &[&str] = &[
        "insert", "update", "delete", "drop", "alter", "create", "into", "pragma", "attach",
        "detach", "vacuum", "reindex", "begin", "commit", "rollback", "savepoint", "release",
        "truncate",
    ];
    for tok in lower.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        if BANNED.contains(&tok) {
            return Err(format!("'{tok}' is not allowed — this tool runs read-only SELECT queries only"));
        }
    }
    Ok(s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gbp_formats_with_thousands_and_sign() {
        assert_eq!(gbp(0), "£0.00");
        assert_eq!(gbp(104_250), "£1,042.50");
        assert_eq!(gbp(123_456_789), "£1,234,567.89");
        assert_eq!(gbp(-83_232), "-£832.32");
        assert_eq!(gbp(5), "£0.05");
    }

    #[test]
    fn debt_renders_as_words_not_signs() {
        assert_eq!(balance_words(-83_232), "overdrawn £832.32");
        assert_eq!(owed_words(-231_000), "owed £2,310.00");
        assert_eq!(owed_words(231_000), "owed £2,310.00");
    }

    #[test]
    fn md_table_escapes_pipes() {
        let t = md_table(&["A", "B"], &[vec!["x|y".to_string(), "z".to_string()]]);
        assert!(t.contains("x\\|y"));
        assert!(t.starts_with("| A | B |\n|---|---|\n"));
    }

    #[test]
    fn validate_query_accepts_selects() {
        assert!(validate_query("SELECT * FROM v_accounts").is_ok());
        assert!(validate_query("  select sum(signed_pounds) from v_transactions where date >= '2026-01-01';").is_ok());
        assert!(validate_query(
            "WITH m AS (SELECT substr(date,1,7) mo, sum(signed_pounds) s FROM v_transactions GROUP BY mo) SELECT * FROM m"
        )
        .is_ok());
    }

    #[test]
    fn validate_query_rejects_writes_and_tricks() {
        assert!(validate_query("DELETE FROM v_accounts").is_err());
        assert!(validate_query("SELECT 1; DROP TABLE accounts").is_err());
        assert!(validate_query("INSERT INTO settings VALUES ('a','b')").is_err());
        assert!(validate_query("SELECT 1 -- sneaky").is_err());
        assert!(validate_query("PRAGMA journal_mode=DELETE").is_err());
        assert!(validate_query("WITH x AS (SELECT 1) INSERT INTO settings SELECT * FROM x").is_err());
        // banned word inside a string literal is fine
        assert!(validate_query("SELECT * FROM v_transactions WHERE description LIKE '%update%'").is_ok());
        // ; inside a string literal is fine
        assert!(validate_query("SELECT * FROM v_transactions WHERE description = 'a;b'").is_ok());
    }
}
