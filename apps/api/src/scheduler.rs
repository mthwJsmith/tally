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
