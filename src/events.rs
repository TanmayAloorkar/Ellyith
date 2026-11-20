use crate::moderation::jobs::{self, JobKind};
use crate::logging::{audit, messages};
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
        serenity::FullEvent::GuildMemberAddition { new_member } => {
            check_member(ctx, data, new_member).await;
            audit::on_join(ctx, data, new_member).await
        }
        serenity::FullEvent::GuildMemberRemoval { guild_id, user, member_data_if_available } => {
            audit::on_leave(ctx, data, *guild_id, user, member_data_if_available.as_ref()).await
        }
        serenity::FullEvent::VoiceStateUpdate { old, new } => audit::on_voice(ctx, data, old.as_ref(), new).await,
        serenity::FullEvent::GuildAuditLogEntryCreate { entry, guild_id } => {
            audit::on_entry(ctx, data, *guild_id, entry).await
        }
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
        serenity::FullEvent::Message { new_message } => messages::on_message(data, new_message),
        serenity::FullEvent::MessageUpdate { new, event, .. } => {
            messages::on_update(&ctx.http, data, new.as_ref(), event).await
        }
        serenity::FullEvent::MessageDelete { channel_id, deleted_message_id, guild_id } => {
            messages::on_delete(&ctx.http, data, channel_id.get(), deleted_message_id.get(), *guild_id).await
        }
        serenity::FullEvent::MessageDeleteBulk { channel_id, multiple_deleted_messages_ids, guild_id } => {
            let ids: Vec<u64> = multiple_deleted_messages_ids.iter().map(|i| i.get()).collect();
            messages::on_bulk_delete(&ctx.http, data, channel_id.get(), &ids, *guild_id).await
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
