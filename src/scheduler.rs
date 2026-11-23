use crate::moderation::actions::record_or_log;
use crate::moderation::cases::{Actor, CaseKind, NewCase};
use crate::moderation::jobs::{self, Job, JobKind};
use crate::moderation::perms::is_unknown_ban;
use crate::Data;
use chrono::Utc;
use poise::serenity_prelude::{self as serenity, GuildId, UserId};
use std::time::Duration;

const REASON: &str = "Tempban expired";

/// Runs due jobs every 30 s. The first tick is immediate, so jobs missed while offline run at startup.
pub async fn run(ctx: serenity::Context, data: Data) {
    let mut tick = tokio::time::interval(Duration::from_secs(30));
    let mut last_prune: Option<std::time::Instant> = None;
    loop {
        tick.tick().await;
        // Ban records only matter for 30 days; prune once a day.
        if last_prune.is_none_or(|t| t.elapsed() >= Duration::from_secs(24 * 3600)) {
            last_prune = Some(std::time::Instant::now());
            if let Err(e) = crate::protection::store::prune_bans(&data.db, Utc::now() - chrono::Duration::days(30)).await {
                tracing::warn!("scheduler: pruning ban records failed: {e}");
            }
        }
        match jobs::due(&data.db, Utc::now()).await {
            Ok(list) => {
                for job in list {
                    run_job(&ctx, &data, job).await;
                }
            }
            Err(e) => tracing::warn!("scheduler: loading due jobs failed: {e}"),
        }
    }
}

async fn run_job(ctx: &serenity::Context, data: &Data, job: Job) {
    let JobKind::Unban = job.kind;
    let (g, t) = (GuildId::new(job.guild_id as u64), UserId::new(job.target_id as u64));
    match ctx.http.remove_ban(g, t, Some(REASON)).await {
        Ok(()) => {
            let case = NewCase {
                guild_id: job.guild_id,
                kind: CaseKind::Unban,
                target_id: job.target_id,
                actor: Actor::System,
                reason: Some(REASON.into()),
                duration_secs: None,
                expires_at: None,
                dm_failed: false,
            };
            if let Err(e) = record_or_log(ctx, data, None, case).await {
                tracing::warn!("scheduler: unban case for job {} failed: {e}", job.id);
                return;
            }
        }
        // Already unbanned by hand: nothing to do, no case.
        Err(e) if is_unknown_ban(&e) => {}
        Err(e) => {
            tracing::warn!("scheduler: unban for job {} failed, will retry: {e}", job.id);
            return;
        }
    }
    if let Err(e) = jobs::mark_done(&data.db, job.id).await {
        tracing::warn!("scheduler: marking job {} done failed: {e}", job.id);
    }
}
