use crate::invoker::Invoker;
use crate::moderation::purge::{self, parse_amount};
use crate::reply;
use crate::{Context, Error};
use poise::serenity_prelude::{GuildChannel, User};

/// Delete recent messages by count (1-1000) or time (like 10m, 2h; up to 14 days).
#[poise::command(slash_command, guild_only)]
pub async fn purge(
    ctx: Context<'_>,
    amount: String,
    #[channel_types("Text", "News")] channel: Option<GuildChannel>,
    member: Option<User>,
    reason: Option<String>,
) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    let inv = Invoker::from_poise(ctx)?;
    let amount = parse_amount(&amount)?;
    let channel = channel.map(|c| c.id).unwrap_or_else(|| ctx.channel_id());
    reply::send(ctx, purge::run(&inv, channel, amount, member.map(|u| u.id), reason).await?).await
}
