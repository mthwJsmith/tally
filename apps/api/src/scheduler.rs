//! Background scheduler that periodically iterates enabled consents and runs the importer.
//!
//! Default schedule: every hour at minute 7 (chosen to dodge other typical cron-on-the-hour load).
//! Configurable via `TALLY_SYNC_CRON` env var (5- or 6-field cron expression).

use crate::importer::Importer;
use crate::AppState;
use anyhow::Result;
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
    sched.start().await?;
    info!(cron = %cron_expr, "scheduler started");
    Ok(sched)
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
