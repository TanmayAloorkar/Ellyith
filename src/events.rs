use crate::moderation::jobs::{self, JobKind};
use crate::moderation::nickname;
use crate::{Data, Error};
use poise::serenity_prelude as serenity;

pub async fn event_handler(
    ctx: &serenity::Context,
    event: &serenity::FullEvent,
    _framework: poise::FrameworkContext<'_, Data, Error>,
    data: &Data,
) -> Result<(), Error> {
    if let serenity::FullEvent::Ready { data_about_bot, .. } = event {
        tracing::info!("connected as {}", data_about_bot.user.name);
    }
    // Unbanned by hand (or by the scheduler itself): the pending job is moot.
    if let serenity::FullEvent::GuildBanRemoval { guild_id, unbanned_user, .. } = event {
        jobs::cancel(&data.db, guild_id.get() as i64, JobKind::Unban, unbanned_user.id.get() as i64).await?;
    }
    // Enforcement failures are logged, never propagated out of the handler.
    match event {
        serenity::FullEvent::GuildMemberAddition { new_member } => check_member(ctx, data, new_member).await,
        serenity::FullEvent::GuildMemberUpdate { old_if_available, new, event } => {
            let now = event.nick.as_deref().or(event.user.global_name.as_deref()).unwrap_or(&event.user.name);
            // No old copy: cannot tell what changed, so check anyway.
            if old_if_available.as_ref().is_none_or(|o| o.display_name() != now) {
                match new {
                    Some(m) => check_member(ctx, data, m).await,
                    None => match event.guild_id.member(&ctx.http, event.user.id).await {
                        Ok(m) => check_member(ctx, data, &m).await,
                        Err(e) => tracing::warn!("nickname check: member fetch failed: {e}"),
                    },
                }
            }
        }
        _ => {}
    }
    Ok(())
}

async fn check_member(ctx: &serenity::Context, data: &Data, member: &serenity::Member) {
    if let Err(e) = nickname::enforce(ctx, data, member).await {
        tracing::warn!("nickname enforcement failed for {}: {e}", member.user.id);
    }
}
