use crate::{Data, Error};
use poise::serenity_prelude as serenity;

pub async fn event_handler(
    _ctx: &serenity::Context,
    event: &serenity::FullEvent,
    _framework: poise::FrameworkContext<'_, Data, Error>,
    _data: &Data,
) -> Result<(), Error> {
    if let serenity::FullEvent::Ready { data_about_bot, .. } = event {
        tracing::info!("connected as {}", data_about_bot.user.name);
    }
    Ok(())
}
