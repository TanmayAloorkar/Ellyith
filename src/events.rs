use crate::moderation::jobs::{self, JobKind};
use crate::{Data, Error};
use poise::serenity_prelude as serenity;

pub async fn event_handler(
    _ctx: &serenity::Context,
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
    Ok(())
}
