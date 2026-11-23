pub mod duration;
pub mod text;
pub mod events;
pub mod settings;
pub mod moderation;
pub mod logging;
pub mod scheduler;
pub mod invoker;
pub mod reply;
pub mod panel;
pub mod safety;
pub mod automod;
pub mod protection;

#[derive(Clone)]
pub struct Data {
    pub db: sqlx::PgPool,
    pub home_guild: poise::serenity_prelude::GuildId,
    pub msg_cache: std::sync::Arc<logging::cache::MessageCache>,
    pub automod: std::sync::Arc<automod::engine::AutoMod>,
}
pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Context<'a> = poise::Context<'a, Data, Error>;

/// An error whose message is safe and meant to be shown to the command user.
#[derive(Debug)]
pub struct UserError(pub String);

impl std::fmt::Display for UserError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for UserError {}

pub fn all_commands() -> Vec<poise::Command<Data, Error>> {
    {
        use moderation::commands::{cases, punish::*};
        vec![ping(), moderation::commands::config::config(), warn(), timeout(), untimeout(), kick(), ban(), tempban(), unban(), cases::case(), cases::history(), cases::note(), cases::mywarnings(), moderation::commands::nick::nick(), moderation::commands::voice::voice(), moderation::commands::purge::purge(), safety::commands::quarantine(), safety::lockdown::lockdown(), automod::commands::automod(), healthcheck()]
    }
}

/// Check how well the server is set up for safety.
#[poise::command(slash_command, guild_only, default_member_permissions = "MANAGE_GUILD")]
async fn healthcheck(ctx: Context<'_>) -> Result<(), Error> {
    ctx.defer_ephemeral().await?;
    reply::send(ctx, safety::health::health_core(&invoker::Invoker::from_poise(ctx)?).await?).await
}

/// Check that the bot is alive.
#[poise::command(slash_command)]
async fn ping(ctx: Context<'_>) -> Result<(), Error> {
    ctx.say("Pong").await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn registers_quarantine() {
        assert!(super::all_commands().iter().any(|c| c.name == "quarantine"));
    }

    #[test]
    fn registers_automod() {
        assert!(super::all_commands().iter().any(|c| c.name == "automod"));
    }

    #[test]
    fn registers_lockdown() {
        assert!(super::all_commands().iter().any(|c| c.name == "lockdown"));
    }

    /// Discord allows command > group > subcommand only; deeper nesting fails registration at startup.
    #[test]
    fn nesting_fits_discord() {
        for c in super::all_commands() {
            for g in &c.subcommands {
                for s in &g.subcommands {
                    assert!(s.subcommands.is_empty(), "/{} {} {} nests too deep", c.name, g.name, s.name);
                }
            }
        }
    }
}
