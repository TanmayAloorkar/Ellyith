use crate::duration::{parse_duration, MAX_TIMEOUT_SECS};
use crate::invoker::Invoker;
use crate::moderation::actions::{execute, ActionRequest};
use crate::moderation::cases::{Actor, CaseKind};
use crate::moderation::commands::{action_reply, detail_lines};
use crate::moderation::perms::{authorize, require_member};
use crate::protection::antinuke;
use crate::protection::logic::PunishSource;
use crate::reply::{self, Tone};
use crate::{Context, Error, UserError};
use poise::serenity_prelude::{CreateEmbed, Permissions, User, UserId};

struct Spec {
    required: Permissions,
    needs_member: bool,
}

fn spec(kind: CaseKind) -> Result<Spec, UserError> {
    let (required, needs_member) = match kind {
        CaseKind::Warn => (Permissions::MODERATE_MEMBERS, false),
        CaseKind::Timeout | CaseKind::Untimeout => (Permissions::MODERATE_MEMBERS, true),
        CaseKind::Kick => (Permissions::KICK_MEMBERS, true),
        CaseKind::Ban | CaseKind::Tempban | CaseKind::Unban => (Permissions::BAN_MEMBERS, false),
        other => return Err(UserError(format!("{} is not a punishment command.", other.label()))),
    };
    Ok(Spec { required, needs_member })
}

fn validate(kind: CaseKind, reason: Option<&str>, duration: Option<chrono::Duration>, ban_delete_days: u8) -> Result<(), UserError> {
    if ban_delete_days > 7 {
        return Err(UserError("delete_days must be between 0 and 7.".into()));
    }
    match kind {
        CaseKind::Warn if reason.is_none_or(str::is_empty) => Err(UserError("A warning needs a reason.".into())),
        CaseKind::Timeout => match duration {
            None => Err(UserError("Timeout needs a duration.".into())),
            Some(d) if d.num_seconds() > MAX_TIMEOUT_SECS => Err(UserError("Timeout can be at most 28 days.".into())),
            Some(_) => Ok(()),
        },
        // Reject up front what `Utc::now() + d` cannot represent.
        CaseKind::Tempban => match duration {
            None => Err(UserError("Tempban needs a duration.".into())),
            Some(d) if chrono::Utc::now().checked_add_signed(d).is_none() => Err(UserError("That duration is too long.".into())),
            Some(_) => Ok(()),
        },
        _ => Ok(()),
    }
}

/// Run a punishment (or its undo) and build the styled reply. Authorises like the slash command.
/// A kick or ban from `PunishSource::Command` counts toward anti-nuke against the invoker.
pub async fn punish(
    inv: &Invoker<'_>,
    src: PunishSource,
    kind: CaseKind,
    user: UserId,
    reason: Option<String>,
    duration: Option<chrono::Duration>,
    ban_delete_days: u8,
) -> Result<CreateEmbed, Error> {
    let spec = spec(kind)?;
    validate(kind, reason.as_deref(), duration, ban_delete_days)?;
    authorize(inv, spec.required, Some(user)).await?;
    if spec.needs_member {
        require_member(inv, user).await?;
    }
    let duration = matches!(kind, CaseKind::Timeout | CaseKind::Tempban).then_some(duration).flatten();
    let case = execute(
        inv.ctx,
        inv.data,
        ActionRequest {
            guild_id: inv.guild_id,
            target: user,
            kind,
            actor: Actor::Moderator(inv.user.id.get() as i64),
            reason: reason.clone(),
            duration,
            ban_delete_days,
            log_case: true,
        },
    )
    .await?;
    if let Some(k) = antinuke::nuke_kind(src, kind) {
        antinuke::count_command(inv.ctx, inv.data, inv.guild_id, inv.user.id, k, user).await;
    }
    action_reply(inv, Tone::Members, kind, user, detail_lines(reason.as_deref(), duration), case.as_ref()).await
}

/// Defer first: the DM and Discord calls can exceed the 3 s response window.
async fn run(ctx: Context<'_>, kind: CaseKind, user: &User, reason: Option<String>, duration: Option<chrono::Duration>, days: u8) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let inv = Invoker::from_poise(ctx)?;
    reply::send(ctx, punish(&inv, PunishSource::Command, kind, user.id, reason, duration, days).await?).await
}

/// Warn a member.
#[poise::command(slash_command, guild_only)]
pub async fn warn(ctx: Context<'_>, user: User, reason: String) -> Result<(), Error> {
    run(ctx, CaseKind::Warn, &user, Some(reason), None, 0).await
}

/// Time a member out.
#[poise::command(slash_command, guild_only)]
pub async fn timeout(ctx: Context<'_>, user: User, duration: String, reason: Option<String>) -> Result<(), Error> {
    let d = parse_duration(&duration)?;
    run(ctx, CaseKind::Timeout, &user, reason, Some(d), 0).await
}

/// Remove a member's timeout.
#[poise::command(slash_command, guild_only)]
pub async fn untimeout(ctx: Context<'_>, user: User, reason: Option<String>) -> Result<(), Error> {
    run(ctx, CaseKind::Untimeout, &user, reason, None, 0).await
}

/// Kick a member.
#[poise::command(slash_command, guild_only)]
pub async fn kick(ctx: Context<'_>, user: User, reason: Option<String>) -> Result<(), Error> {
    run(ctx, CaseKind::Kick, &user, reason, None, 0).await
}

/// Ban a user, even if they are not in the server.
#[poise::command(slash_command, guild_only)]
pub async fn ban(
    ctx: Context<'_>,
    user: User,
    reason: Option<String>,
    #[description = "Days of messages to delete (0-7)"] delete_days: Option<u8>,
) -> Result<(), Error> {
    run(ctx, CaseKind::Ban, &user, reason, None, delete_days.unwrap_or(0)).await
}

/// Ban a user for a limited time, then unban automatically.
#[poise::command(slash_command, guild_only)]
pub async fn tempban(
    ctx: Context<'_>,
    user: User,
    duration: String,
    reason: Option<String>,
    #[description = "Days of messages to delete (0-7)"] delete_days: Option<u8>,
) -> Result<(), Error> {
    let d = parse_duration(&duration)?;
    run(ctx, CaseKind::Tempban, &user, reason, Some(d), delete_days.unwrap_or(0)).await
}

/// Unban a user (paste their ID).
#[poise::command(slash_command, guild_only)]
pub async fn unban(ctx: Context<'_>, user: User, reason: Option<String>) -> Result<(), Error> {
    run(ctx, CaseKind::Unban, &user, reason, None, 0).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn validation() {
        let d = |s| Some(Duration::seconds(s));
        assert!(validate(CaseKind::Warn, None, None, 0).is_err());
        assert!(validate(CaseKind::Warn, Some("x"), None, 0).is_ok());
        assert!(validate(CaseKind::Timeout, None, None, 0).is_err());
        assert!(validate(CaseKind::Timeout, None, d(MAX_TIMEOUT_SECS + 1), 0).is_err());
        assert!(validate(CaseKind::Timeout, None, d(MAX_TIMEOUT_SECS), 0).is_ok());
        assert!(validate(CaseKind::Tempban, None, None, 0).is_err());
        assert!(validate(CaseKind::Tempban, None, d(60), 7).is_ok());
        assert!(validate(CaseKind::Ban, None, None, 8).is_err());
        assert!(spec(CaseKind::VoiceMute).is_err());
    }
}
