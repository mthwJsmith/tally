//! Background scheduler that periodically iterates enabled consents and runs the importer.
//!
//! Default schedule: every hour at minute 7 (chosen to dodge other typical cron-on-the-hour load).
//! Configurable via `TALLY_SYNC_CRON` env var (5- or 6-field cron expression).

use crate::importer::Importer;
use crate::AppState;
use anyhow::Result;
use chrono::{TimeZone, Utc};
use std::sync::Arc;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info};

pub async fn start_scheduler(state: Arc<AppState>) -> Result<JobScheduler> {
    let sched = JobScheduler::new().await?;

    let cron_expr = std::env::var("TALLY_SYNC_CRON")
        .unwrap_or_else(|_| "0 7 * * * *".to_string()); // sec min hour dom mon dow — every hour at :07

    let state_clone = state.clone();
    let job = Job::new_async(cron_expr.as_str(), move |_uuid, _l| {
        let state = state_clone.clone();
        Box::pin(async move {
            run_all_consents(state).await;
        })
    })?;

    sched.add(job).await?;

    // Reminders: hourly check for due/unticked reminders + period rollover.
    let state_clone = state.clone();
    let reminders_job = Job::new_async("0 15 * * * *", move |_uuid, _l| {
        let state = state_clone.clone();
        Box::pin(async move {
            check_reminders(state).await;
        })
    })?;
    sched.add(reminders_job).await?;

    // Watchlist: poll deal sources every 3 hours (at :25).
    let state_clone = state.clone();
    let watchlist_job = Job::new_async("0 25 */3 * * *", move |_uuid, _l| {
        let state = state_clone.clone();
        Box::pin(async move {
            poll_watchlist(state).await;
        })
    })?;
    sched.add(watchlist_job).await?;

    // Direct debits due soon: daily Telegram digest at 08:00.
    let state_clone = state.clone();
    let bills_job = Job::new_async("0 0 8 * * *", move |_uuid, _l| {
        let state = state_clone.clone();
        Box::pin(async move {
            notify_due_bills(state).await;
        })
    })?;
    sched.add(bills_job).await?;

    sched.start().await?;
    info!(cron = %cron_expr, "scheduler started");
    Ok(sched)
}

/// Hourly: ping reminders that are due-and-unticked, then roll elapsed periods forward.
pub async fn check_reminders(state: Arc<AppState>) {
    let now = Utc::now().timestamp();

    match state.db.reminders_to_notify(now).await {
        Ok(rs) => {
            for r in rs {
                let text = format!("⏰ *Reminder:* {} — due {}", r.title, fmt_day(r.due_at));
                state.notifier.send_telegram_text(&text, false).await;
                let _ = state.db.mark_reminder_notified(r.id, now).await;
            }
        }
        Err(e) => error!("reminders_to_notify failed: {e:#}"),
    }

    match state.db.reminders_to_roll(now).await {
        Ok(rs) => {
            for r in rs {
                // Advance the deadline until it's in the future (handles long-dormant reminders).
                let mut due = crate::recurrence::next_occurrence(&r.freq, r.every_n, r.anchor_day, r.due_at);
                while due <= now {
                    due = crate::recurrence::next_occurrence(&r.freq, r.every_n, r.anchor_day, due);
                }
                let _ = state.db.roll_reminder(r.id, due).await;
            }
        }
        Err(e) => error!("reminders_to_roll failed: {e:#}"),
    }
}

fn fmt_day(ts: i64) -> String {
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|d| d.format("%a %e %b").to_string())
        .unwrap_or_default()
}

/// Daily: Telegram digest of direct debits / bills due in the next few days.
pub async fn notify_due_bills(state: Arc<AppState>) {
    let days = 5;
    let bills = match state.db.list_bills_due_within(days).await {
        Ok(b) => b,
        Err(e) => {
            error!("notify_due_bills: {e:#}");
            return;
        }
    };
    if bills.is_empty() {
        return;
    }
    let mut lines = vec![format!("💸 *Direct debits due in {days} days:*")];
    for b in bills {
        let when = b.next_expected_date.map(fmt_day).unwrap_or_default();
        let amt = if b.expected_amount_max_cents > 0 {
            format!(" ~£{:.2}", b.expected_amount_max_cents as f64 / 100.0)
        } else {
            String::new()
        };
        lines.push(format!("• {}{} ({})", b.name, amt, when));
    }
    state
        .notifier
        .send_telegram_text(&lines.join("\n"), false)
        .await;
}

/// Every few hours: poll each watchlist item's sources, store new deals, alert on under-target.
pub async fn poll_watchlist(state: Arc<AppState>) {
    let items = match state.db.list_watchlist().await {
        Ok(i) => i,
        Err(e) => {
            error!("list_watchlist failed: {e:#}");
            return;
        }
    };
    for item in items {
        let sources = match state.db.watchlist_sources(item.id).await {
            Ok(s) => s,
            Err(e) => {
                error!("watchlist_sources failed: {e:#}");
                continue;
            }
        };
        for src in sources {
            let found = match src.kind.as_str() {
                "rss" => state.deals.fetch_rss(&src.ref_).await.unwrap_or_else(|e| {
                    error!("rss {}: {e:#}", src.ref_);
                    vec![]
                }),
                "changedetection" => match state.deals.fetch_changedetection(&src.ref_).await {
                    Ok(Some(d)) => vec![d],
                    Ok(None) => vec![],
                    Err(e) => {
                        error!("changedetection {}: {e:#}", src.ref_);
                        vec![]
                    }
                },
                _ => vec![],
            };
            for d in found {
                let inserted = state
                    .db
                    .insert_observation_if_new(
                        item.id,
                        &d.title,
                        d.url.as_deref(),
                        d.price_cents,
                        &src.kind,
                        &d.guid,
                    )
                    .await;
                let Ok(Some(obs_id)) = inserted else { continue }; // dup or error -> skip
                // Alert when there's no target, or a parsed price is at/under it.
                let alert = match (item.target_price_cents, d.price_cents) {
                    (Some(t), Some(p)) => p <= t,
                    (Some(_), None) => false,
                    (None, _) => true,
                };
                if alert {
                    let price = d
                        .price_cents
                        .map(|c| format!(" £{:.2}", c as f64 / 100.0))
                        .unwrap_or_default();
                    let link = d.url.as_deref().unwrap_or("");
                    let text = format!("💰 *Deal* ({}):{} {}\n{}", item.name, price, d.title, link);
                    state.notifier.send_telegram_text(&text, false).await;
                    let _ = state.db.mark_deal_notified(obs_id).await;
                }
            }
        }
    }
}

pub async fn run_all_consents(state: Arc<AppState>) {
    let consents = match state.db.list_enabled_consents().await {
        Ok(c) => c,
        Err(e) => {
            error!("scheduler: failed to list consents: {e:#}");
            return;
        }
    };
    info!("scheduled tick — syncing {} consent(s)", consents.len());

    let importer = Importer {
        db: state.db.clone(),
        tl: state.tl.clone(),
        notifier: state.notifier.clone(),
        ai: state.ai.clone(),
    };

    for consent in consents {
        match importer.sync_consent(&consent).await {
            Ok(r) => info!(
                consent = %consent.nickname,
                imported = r.transactions_imported,
                "consent sync ok"
            ),
            Err(e) => error!(consent = %consent.nickname, "consent sync failed: {e:#}"),
        }
    }
}
