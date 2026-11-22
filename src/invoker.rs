use crate::{Context, Data, Error, UserError};
use poise::serenity_prelude::{self as serenity, GuildId, User};

/// Who is acting, in which guild. Built from a slash command or (later) a component interaction.
pub struct Invoker<'a> {
    pub ctx: &'a serenity::Context,
    pub data: &'a Data,
    pub guild_id: GuildId,
    pub user: User,
}

impl<'a> Invoker<'a> {
    pub fn from_poise(ctx: Context<'a>) -> Result<Self, Error> {
        let guild_id = ctx.guild_id().ok_or_else(|| UserError("This command only works in a server.".into()))?;
        Ok(Self { ctx: ctx.serenity_context(), data: ctx.data(), guild_id, user: ctx.author().clone() })
    }
}
