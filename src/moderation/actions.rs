use crate::duration::{format_duration, MAX_TIMEOUT_SECS};
use crate::logging::{case_embed, send_log, CaseEvent, LogCategory};
use crate::moderation::cases::{self, Actor, Case, CaseKind, NewCase};
use crate::moderation::ladder::{self, LadderAction, LadderRule};
use crate::moderation::perms::{is_absent, is_exempt_from_auto, member_facts};
use crate::moderation::templates::{self, dm_kinds, TemplateVars};
use crate::text::truncate;
use crate::{settings, Data, Error, UserError};
use chrono::{Duration, Utc};
use poise::serenity_prelude::{self as serenity, CreateEmbed, CreateMessage, EditMember, GuildId, Permissions, Timestamp, UserId};

pub struct ActionRequest {
    pub guild_id: GuildId,
    pub target: UserId,
    pub kind: CaseKind,
    pub actor: Actor,
    pub reason: Option<String>,
    pub duration: Option<Duration>,
    pub ban_delete_days: u8,
}

/// Map a fired ladder rule to the action to run.
fn ladder_request(guild_id: GuildId, target: UserId, rule: &LadderRule, warns: i64) -> ActionRequest {
    ActionRequest {
        guild_id,
        target,
        kind: match rule.action {
            LadderAction::Timeout => CaseKind::Timeout,
            LadderAction::Kick => CaseKind::Kick,
            LadderAction::Tempban => CaseKind::Tempban,
            LadderAction::Ban => CaseKind::Ban,
        },
        actor: Actor::AutoPunishment,
        reason: Some(format!("Reached {warns} warnings")),
        duration: rule.duration_secs.map(Duration::seconds),
        ban_delete_days: 0,
    }
}

/// Refuse before any DM when Discord would reject the action anyway.
fn bot_can_apply(kind: CaseKind, bot: Permissions, target_is_admin_or_owner: bool) -> Result<(), &'static str> {
    let need = match kind {
        CaseKind::Timeout | CaseKind::Untimeout => Permissions::MODERATE_MEMBERS,
        CaseKind::Kick => Permissions::KICK_MEMBERS,
        CaseKind::Ban | CaseKind::Unban => Permissions::BAN_MEMBERS,
        _ => Permissions::empty(),
    };
    if !bot.contains(need) {
        return Err("I don't have permission to do that.");
    }
    if kind == CaseKind::Timeout && target_is_admin_or_owner {
        return Err("Discord does not allow timing out administrators.");
    }
    Ok(())
}

async fn precheck(ctx: &serenity::Context, req: &ActionRequest) -> Result<(), Error> {
    if req.kind == CaseKind::Warn {
        return Ok(());
    }
    let bot_id = ctx.cache.current_user().id;
    let bot_m = req.guild_id.member(ctx, bot_id).await?;
    let bot = member_facts(ctx, req.guild_id, &bot_m).await?;
    let mut target_admin = false;
    if req.kind == CaseKind::Timeout {
        match req.guild_id.member(ctx, req.target).await {
            Ok(m) => {
                let f = member_facts(ctx, req.guild_id, &m).await?;
                target_admin = f.is_owner || f.permissions.contains(Permissions::ADMINISTRATOR);
            }
            Err(e) if is_absent(&e) => {}
            Err(e) => return Err(e.into()),
        }
    }
    bot_can_apply(req.kind, bot.permissions, target_admin).map_err(|m| UserError(m.into()).into())
}

fn http_error_message(status: Option<u16>) -> &'static str {
    match status {
        Some(403) => "I don't have permission to do that.",
        _ => "Discord rejected that action.",
    }
}

/// Discord HTTP errors become moderator-facing messages; anything else passes through.
fn discord_error(e: serenity::Error) -> Error {
    match &e {
        serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(r)) => {
            UserError(http_error_message(Some(r.status_code.as_u16())).into()).into()
        }
        _ => e.into(),
    }
}

/// Returns true when the DM could not be delivered.
async fn send_dm(ctx: &serenity::Context, data: &Data, req: &ActionRequest, number: i32) -> Result<bool, Error> {
    let gid = req.guild_id.get() as i64;
    let s = settings::get_or_create(&data.db, gid).await?;
    let template = match templates::get(&data.db, gid, req.kind).await? {
        Some(t) => t,
        None => templates::default_template(req.kind).to_string(),
    };
    let server = ctx.cache.guild(req.guild_id).map(|g| g.name.clone()).unwrap_or_else(|| "the server".into());
    let moderator = match req.actor {
        Actor::Moderator(id) if s.dm_show_moderator => {
            Some(UserId::new(id as u64).to_user(ctx).await.map(|u| u.name).unwrap_or_else(|_| "Staff".into()))
        }
        _ => None,
    };
    let duration = req.duration.map(format_duration);
    let text = templates::render(
        &template,
        &TemplateVars {
            action: req.kind.label(),
            reason: req.reason.as_deref().filter(|r| !r.is_empty()).unwrap_or("No reason given"),
            duration: duration.as_deref(),
            server: &server,
            case_number: number,
            moderator: moderator.as_deref(),
        },
    );
    Ok(req.target.direct_message(ctx, CreateMessage::new().content(text)).await.is_err())
}

/// Callers have already run `authorize`. Creates no case if the Discord action fails.
pub async fn execute(ctx: &serenity::Context, data: &Data, req: ActionRequest) -> Result<Case, Error> {
    if req.kind == CaseKind::Tempban {
        return Err(UserError("Tempban is not available yet.".into()).into());
    }
    precheck(ctx, &req).await?;
    let gid = req.guild_id.get() as i64;
    let mut number = None;
    let mut dm_failed = false;
    if dm_kinds().contains(&req.kind) {
        let n = cases::reserve_number(&data.db, gid).await?;
        number = Some(n);
        dm_failed = send_dm(ctx, data, &req, n).await?;
    }

    let audit = truncate(req.reason.as_deref().unwrap_or(""), 512);
    let (g, t) = (req.guild_id, req.target);
    let mut expires_at = None;
    match req.kind {
        CaseKind::Warn => {}
        CaseKind::Timeout => {
            let d = req.duration.ok_or_else(|| UserError("Timeout needs a duration.".into()))?;
            if d.num_seconds() < 1 || d.num_seconds() > MAX_TIMEOUT_SECS {
                return Err(UserError("Timeout duration must be between 1 second and 28 days.".into()).into());
            }
            let until = Utc::now() + d;
            expires_at = Some(until);
            let ts = Timestamp::from_unix_timestamp(until.timestamp()).map_err(|e| UserError(e.to_string()))?;
            g.edit_member(ctx, t, EditMember::new().disable_communication_until_datetime(ts).audit_log_reason(&audit))
                .await
                .map_err(discord_error)?;
        }
        CaseKind::Untimeout => {
            g.edit_member(ctx, t, EditMember::new().enable_communication().audit_log_reason(&audit))
                .await
                .map_err(discord_error)?;
        }
        CaseKind::Kick => g.kick_with_reason(ctx, t, &audit).await.map_err(discord_error)?,
        CaseKind::Ban => g.ban_with_reason(ctx, t, req.ban_delete_days.min(7), &audit).await.map_err(discord_error)?,
        CaseKind::Unban => ctx.http.remove_ban(g, t, Some(&audit)).await.map_err(discord_error)?,
        other => return Err(UserError(format!("{} is not available yet.", other.label())).into()),
    }

    let case = record_case(
        ctx,
        data,
        number,
        NewCase {
            guild_id: gid,
            kind: req.kind,
            target_id: t.get() as i64,
            actor: req.actor,
            reason: req.reason,
            duration_secs: req.duration.map(|d| d.num_seconds()),
            expires_at,
            dm_failed,
        },
    )
    .await?;

    if req.kind == CaseKind::Warn {
        if let Err(e) = apply_ladder(ctx, data, g, t).await {
            tracing::warn!("auto punishment failed: {e}");
            let embed = CreateEmbed::new()
                .title("Auto punishment failed")
                .description(format!("Target <@{}> after case #{}: {e}", t.get(), case.case_number));
            send_log(&ctx.http, &data.db, g, LogCategory::ModActions, embed).await;
        }
    }
    Ok(case)
}

pub async fn record_case(ctx: &serenity::Context, data: &Data, number: Option<i32>, c: NewCase) -> Result<Case, Error> {
    let n = match number {
        Some(n) => n,
        None => cases::reserve_number(&data.db, c.guild_id).await?,
    };
    let case = cases::insert(&data.db, n, &c).await?;
    send_log(&ctx.http, &data.db, GuildId::new(c.guild_id as u64), LogCategory::ModActions, case_embed(&case, CaseEvent::Created)).await;
    Ok(case)
}

pub async fn apply_ladder(ctx: &serenity::Context, data: &Data, guild_id: GuildId, target: UserId) -> Result<Option<Case>, Error> {
    let gid = guild_id.get() as i64;
    let s = settings::get_or_create(&data.db, gid).await?;
    match guild_id.member(ctx, target).await {
        Ok(m) => {
            if is_exempt_from_auto(&member_facts(ctx, guild_id, &m).await?, &s) {
                return Ok(None);
            }
        }
        Err(e) if is_absent(&e) => {}
        Err(e) => return Err(e.into()),
    }
    let decay = s.warn_decay_enabled.then_some(s.warn_decay_days);
    let warns = cases::active_warn_count(&data.db, gid, target.get() as i64, decay).await?;
    let rules = ladder::list_rules(&data.db, gid).await?;
    let Some(rule) = ladder::rule_for_count(&rules, warns) else { return Ok(None) };
    let req = ladder_request(guild_id, target, rule, warns);
    Ok(Some(Box::pin(execute(ctx, data, req)).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_maps_to_request() {
        let rule = LadderRule { warn_count: 3, action: LadderAction::Timeout, duration_secs: Some(3600) };
        let r = ladder_request(GuildId::new(1), UserId::new(2), &rule, 3);
        assert_eq!(r.kind, CaseKind::Timeout);
        assert_eq!(r.duration, Some(Duration::seconds(3600)));
        assert_eq!(r.actor, Actor::AutoPunishment);
        assert_eq!(r.reason.as_deref(), Some("Reached 3 warnings"));
        let kick = LadderRule { warn_count: 5, action: LadderAction::Kick, duration_secs: None };
        let r = ladder_request(GuildId::new(1), UserId::new(2), &kick, 5);
        assert_eq!((r.kind, r.duration), (CaseKind::Kick, None));
    }

    #[test]
    fn bot_permission_gate() {
        let all = Permissions::all();
        for (k, p) in [
            (CaseKind::Kick, Permissions::KICK_MEMBERS),
            (CaseKind::Ban, Permissions::BAN_MEMBERS),
            (CaseKind::Unban, Permissions::BAN_MEMBERS),
            (CaseKind::Timeout, Permissions::MODERATE_MEMBERS),
            (CaseKind::Untimeout, Permissions::MODERATE_MEMBERS),
        ] {
            assert_eq!(bot_can_apply(k, all - p, false), Err("I don't have permission to do that."), "{k:?}");
            assert_eq!(bot_can_apply(k, p, false), Ok(()), "{k:?}");
        }
        assert_eq!(bot_can_apply(CaseKind::Warn, Permissions::empty(), false), Ok(()));
        assert!(bot_can_apply(CaseKind::Timeout, all, true).is_err());
        assert_eq!(bot_can_apply(CaseKind::Ban, all, true), Ok(()));
    }

    #[test]
    fn http_messages() {
        assert_eq!(http_error_message(Some(403)), "I don't have permission to do that.");
        assert_eq!(http_error_message(Some(404)), "Discord rejected that action.");
        assert_eq!(http_error_message(None), "Discord rejected that action.");
    }
}
