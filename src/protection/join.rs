//! The join pipeline: raid, then the new account gate, then alt scoring. Verification runs after, in events.rs.
use super::logic::{decide_join, gate_blocks, JoinOutcome};
use super::{alts, raid, store, EMOJI_GATE};
use crate::moderation::actions::{execute_with_dm, ActionRequest};
use crate::moderation::cases::{Actor, CaseKind};
use crate::safety::quarantine::{self, QuarantineReason};
use crate::{Data, UserError};
use chrono::{DateTime, Utc};
use poise::serenity_prelude::{self as serenity, Member};
use std::time::Instant;

pub const GATE_REASON: &str = "New account gate";

pub fn gate_dm_text(days: i32, created: DateTime<Utc>) -> String {
    let after = created + chrono::Duration::days(days.into());
    format!("This server needs accounts older than {days} days. Come back after <t:{}:D>.", after.timestamp())
}

/// True when the member was quarantined, kicked or banned here; flag-only outcomes return false.
pub async fn on_join(ctx: &serenity::Context, data: &Data, member: &Member) -> bool {
    if member.user.bot {
        return false;
    }
    let (guild, user) = (member.guild_id, member.user.id);
    let gid = guild.get() as i64;
    let s = match store::get_settings(&data.db, gid).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!("protection settings for {guild} failed: {e}");
            return false;
        }
    };
    // The window comes first: a burst must not wait behind the invite fetch.
    let burst = {
        let mut joins = data.protection.joins.lock().unwrap_or_else(|e| e.into_inner());
        joins.entry(guild.get()).or_default().push(Instant::now(), user.get(), s.raid_joins.max(1) as usize, s.raid_secs.max(1) as u64)
    };
    let mut active = store::active_raid(&data.db, gid).await.unwrap_or_else(|e| {
        tracing::warn!("active raid lookup for {guild} failed: {e}");
        None
    });
    // shortcut: no invite tracking during a raid or burst (one serialized HTTP fetch per join); the invite is recorded as unknown.
    let invite = if active.is_none() && burst.is_none() { data.protection.invites.resolve_join(ctx, guild).await } else { None };
    let (code, inviter) = (invite.as_ref().map(|i| i.0.as_str()), invite.as_ref().and_then(|i| i.1).map(|u| u as i64));
    if let Err(e) = store::record_member_invite(&data.db, gid, user.get() as i64, code, inviter).await {
        tracing::warn!("recording invite of {user} failed: {e}");
    }

    match (&active, burst) {
        // Only a new burst restarts the auto-end; single joins during a raid do not.
        (Some(r), Some(_)) => {
            if let Err(e) = raid::extend(data, r).await {
                tracing::warn!("extending raid in {guild} failed: {e}");
            }
        }
        (None, Some(ids)) if s.raid_enabled => {
            active = match raid::start(ctx, data, guild, Actor::AutoPunishment).await {
                Ok(r) => {
                    // The burst's earlier joins become raiders too, in the background so this join is not held up.
                    let (ctx, data, r2) = (ctx.clone(), data.clone(), r.clone());
                    tokio::spawn(async move { raid::on_burst(&ctx, &data, guild, ids, user, &r2).await });
                    Some(r)
                }
                Err(e) => {
                    // Lost a race with another join's start: that raid is the one to join.
                    if !e.is::<UserError>() {
                        tracing::warn!("starting raid in {guild} failed: {e}");
                    }
                    store::active_raid(&data.db, gid).await.ok().flatten()
                }
            }
        }
        _ => {}
    }

    // A raider the raid responses did not remove (alert or lockdown only, or a failed action) still meets the gate and alt checks.
    if let Some(r) = &active {
        if raid::on_raider(ctx, data, guild, member, r).await {
            return true;
        }
    }

    let created = member.user.created_at().unix_timestamp();
    let age_days = (Utc::now().timestamp() - created).max(0) / 86_400;
    let gate = gate_blocks(s.gate_enabled, s.gate_days, age_days);
    // Scored only when it can matter: the gate comes first.
    let score = if !gate && s.alt_enabled { Some(alts::score_member(ctx, data, member).await) } else { None };
    let alt_over = score.as_ref().is_some_and(|a| a.total >= s.alt_threshold);
    match decide_join(false, gate, alt_over, false, false) {
        JoinOutcome::Gate => {
            let created = DateTime::from_timestamp(created, 0).unwrap_or_else(Utc::now);
            match s.gate_action.as_str() {
                "flag" => {
                    let title = format!("{EMOJI_GATE} New account · {age_days} days old");
                    let lines = [("User", format!("<@{user}> (@{})", member.user.name)), ("Account made", format!("<t:{created}:R>", created = created.timestamp()))];
                    alts::post_card(ctx, data, guild, user, &title, &lines).await;
                    false
                }
                "kick" | "ban" => {
                    let kind = if s.gate_action == "ban" { CaseKind::Ban } else { CaseKind::Kick };
                    let req = ActionRequest { guild_id: guild, target: user, kind, actor: Actor::AutoPunishment, reason: Some(GATE_REASON.into()), duration: None, ban_delete_days: 0, log_case: true };
                    // One DM, the gate text, sent by execute only once its precheck passes, before the kick/ban.
                    match execute_with_dm(ctx, data, req, Some(gate_dm_text(s.gate_days, created))).await {
                        Ok(_) => true,
                        Err(e) => {
                            tracing::warn!("gate {} of {user} failed: {e}", kind.as_str());
                            false
                        }
                    }
                }
                _ => quarantine_as(ctx, data, member, QuarantineReason::Gate, GATE_REASON).await,
            }
        }
        JoinOutcome::Alt => {
            if s.alt_response == "quarantine" {
                quarantine_as(ctx, data, member, QuarantineReason::Alt, alts::REASON).await
            } else {
                if let Some(score) = &score {
                    alts::post_flag(ctx, data, member, score, invite).await;
                }
                false
            }
        }
        JoinOutcome::Raid | JoinOutcome::Verify | JoinOutcome::Pass => false,
    }
}

async fn quarantine_as(ctx: &serenity::Context, data: &Data, member: &Member, reason: QuarantineReason, note: &str) -> bool {
    match quarantine::enter(ctx, data, member.guild_id, member, reason, Actor::AutoPunishment, Some(note.into()), None).await {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("{note} quarantine of {} failed: {e}", member.user.id);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_dm_copy() {
        let created = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        assert_eq!(gate_dm_text(7, created), "This server needs accounts older than 7 days. Come back after <t:1700604800:D>.");
    }
}
