use aio_bot::{all_commands, events, Data, Error, UserError};
use poise::serenity_prelude as serenity;
use sqlx::postgres::PgPoolOptions;

fn require_env(name: &str) -> String {
    match std::env::var(name) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            eprintln!("Missing required environment variable {name} (set it in .env)");
            std::process::exit(1);
        }
    }
}

async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
    // Setup failed: the bot would stay connected with no user data, so stop and let the host show it.
    if let poise::FrameworkError::Setup { error, .. } = &error {
        tracing::error!("Error in user data setup: {error:?}");
        std::process::exit(1);
    }
    let failed = match error {
        poise::FrameworkError::Command { error, ctx, .. } => Some((error, ctx)),
        poise::FrameworkError::CommandCheckFailed { error: Some(error), ctx, .. } => Some((error, ctx)),
        other => {
            if let Err(e) = poise::builtins::on_error(other).await {
                tracing::error!("error while handling error: {e}");
            }
            None
        }
    };
    if let Some((error, ctx)) = failed {
        let text = match error.downcast_ref::<UserError>() {
            Some(user_error) => user_error.0.clone(),
            None => {
                tracing::error!("command error: {error:?}");
                "Something went wrong. The error was logged.".to_string()
            }
        };
        if let Err(e) = aio_bot::reply::send(ctx, aio_bot::reply::refused(&text, false)).await {
            tracing::error!("failed to send error reply: {e}");
        }
    }
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let token = require_env("DISCORD_TOKEN");
    let database_url = require_env("DATABASE_URL");
    let guild_id: u64 = require_env("GUILD_ID").parse().unwrap_or_else(|_| {
        eprintln!("GUILD_ID must be a numeric guild ID");
        std::process::exit(1);
    });

    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_url)
        .await
        .expect("failed to connect to the database");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("failed to run migrations");

    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: all_commands(),
            // poise 0.7 passes (framework, event); unpack to the 4-arg handler signature.
            event_handler: |framework, event| {
                Box::pin(events::event_handler(
                    framework.serenity_context,
                    event,
                    framework,
                    framework.user_data,
                ))
            },
            on_error: |error| Box::pin(on_error(error)),
            ..Default::default()
        })
        .setup(move |ctx, _ready, framework| {
            Box::pin(async move {
                poise::builtins::register_in_guild(
                    ctx,
                    &framework.options().commands,
                    serenity::GuildId::new(guild_id),
                )
                .await?;
                let data = Data {
                    db: pool,
                    home_guild: serenity::GuildId::new(guild_id),
                    msg_cache: std::sync::Arc::new(aio_bot::logging::cache::MessageCache::new(10_000, std::time::Duration::from_secs(24 * 3600))),
                    automod: std::sync::Arc::new(aio_bot::automod::engine::AutoMod::new()),
                    protection: std::sync::Arc::new(aio_bot::protection::Runtime::new()),
                };
                tokio::spawn(aio_bot::scheduler::run(ctx.clone(), data.clone()));
                tokio::spawn(aio_bot::automod::scam::run(data.clone()));
                Ok(data)
            })
        })
        .build();

    let intents = serenity::GatewayIntents::GUILDS
        | serenity::GatewayIntents::GUILD_MEMBERS
        | serenity::GatewayIntents::GUILD_MESSAGES
        | serenity::GatewayIntents::MESSAGE_CONTENT
        | serenity::GatewayIntents::GUILD_MODERATION
        | serenity::GatewayIntents::GUILD_VOICE_STATES
        | serenity::GatewayIntents::GUILD_INVITES;

    let mut client = serenity::ClientBuilder::new(&token, intents)
        .framework(framework)
        .await
        .expect("failed to build the Discord client");
    if let Err(e) = client.start().await {
        tracing::error!("client stopped: {e}");
        std::process::exit(1);
    }
}
