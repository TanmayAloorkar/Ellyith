pub mod cache;
pub mod messages;

use crate::duration::format_duration;
use crate::moderation::cases::{Actor, Case};
use crate::text::truncate;
use poise::serenity_prelude::{ChannelId, CreateEmbed, CreateEmbedFooter, CreateMessage, GuildId, Http};
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, poise::ChoiceParameter)]
pub enum LogCategory {
    #[name = "mod_actions"]
    ModActions,
    #[name = "messages"]
    Messages,
    #[name = "joins_leaves"]
    JoinsLeaves,
    #[name = "member_updates"]
    MemberUpdates,
    #[name = "channels"]
    Channels,
    #[name = "roles"]
    Roles,
    #[name = "voice"]
    Voice,
    #[name = "external_mod"]
    ExternalMod,
}

impl LogCategory {
    pub const ALL: [LogCategory; 8] = [
        Self::ModActions,
        Self::Messages,
        Self::JoinsLeaves,
        Self::MemberUpdates,
        Self::Channels,
        Self::Roles,
        Self::Voice,
        Self::ExternalMod,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ModActions => "mod_actions",
            Self::Messages => "messages",
            Self::JoinsLeaves => "joins_leaves",
            Self::MemberUpdates => "member_updates",
            Self::Channels => "channels",
            Self::Roles => "roles",
            Self::Voice => "voice",
            Self::ExternalMod => "external_mod",
        }
    }
}

pub async fn set_channel(db: &PgPool, guild_id: i64, cat: LogCategory, channel_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO log_channels (guild_id, category, channel_id) VALUES ($1, $2, $3) \
         ON CONFLICT (guild_id, category) DO UPDATE SET channel_id = EXCLUDED.channel_id",
    )
    .bind(guild_id)
    .bind(cat.as_str())
    .bind(channel_id)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn clear(db: &PgPool, guild_id: i64, cat: LogCategory) -> Result<bool, sqlx::Error> {
    let r = sqlx::query("DELETE FROM log_channels WHERE guild_id = $1 AND category = $2")
        .bind(guild_id)
        .bind(cat.as_str())
        .execute(db)
        .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn list(db: &PgPool, guild_id: i64) -> Result<Vec<(LogCategory, i64)>, sqlx::Error> {
    let rows: Vec<(String, i64)> = sqlx::query_as("SELECT category, channel_id FROM log_channels WHERE guild_id = $1")
        .bind(guild_id)
        .fetch_all(db)
        .await?;
    // Unknown stored categories are skipped; order follows ALL.
    Ok(LogCategory::ALL
        .into_iter()
        .filter_map(|c| rows.iter().find(|(n, _)| n == c.as_str()).map(|(_, id)| (c, *id)))
        .collect())
}

pub async fn channel_for(db: &PgPool, guild_id: i64, cat: LogCategory) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT channel_id FROM log_channels WHERE guild_id = $1 AND category = $2")
        .bind(guild_id)
        .bind(cat.as_str())
        .fetch_optional(db)
        .await
}

/// Post `embed` to the category's channel. No-op when the category is off; never errors.
pub async fn send_log(http: &Http, db: &PgPool, guild_id: GuildId, cat: LogCategory, embed: CreateEmbed) {
    let ch = match channel_for(db, guild_id.get() as i64, cat).await {
        Ok(Some(ch)) => ch,
        Ok(None) => return,
        Err(e) => return tracing::warn!("log channel lookup failed: {e}"),
    };
    if let Err(e) = ChannelId::new(ch as u64).send_message(http, CreateMessage::new().embed(embed)).await {
        tracing::warn!("sending {} log to channel {ch} failed: {e}", cat.as_str());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseEvent {
    Created,
    ReasonEdited,
    Removed { by: i64 },
}

pub fn case_embed(c: &Case, ev: CaseEvent) -> CreateEmbed {
    let actor = match c.actor {
        Actor::Moderator(id) => format!("<@{id}>"),
        Actor::AutoPunishment => "Auto punishment".to_string(),
        Actor::System => "System".to_string(),
    };
    let reason = c.reason.as_deref().filter(|r| !r.is_empty()).unwrap_or("No reason given");
    let mut e = CreateEmbed::new()
        .title(format!("Case #{} · {}", c.case_number, c.kind.label()))
        .field("Target", format!("<@{0}> ({0})", c.target_id), true)
        .field("Moderator", actor, true)
        .field("Reason", truncate(reason, 1024), false);
    if let Some(s) = c.duration_secs {
        e = e.field("Duration", format_duration(chrono::Duration::seconds(s)), true);
    }
    if c.dm_failed {
        e = e.field("Note", "DM failed", false);
    }
    match ev {
        CaseEvent::Created => e,
        CaseEvent::ReasonEdited => e.footer(CreateEmbedFooter::new("Reason edited")),
        CaseEvent::Removed { by } => e.footer(CreateEmbedFooter::new(format!("Removed by <@{by}>"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::moderation::cases::CaseKind;

    fn case() -> Case {
        Case {
            guild_id: 1,
            case_number: 7,
            kind: CaseKind::Timeout,
            target_id: 42,
            actor: Actor::AutoPunishment,
            reason: Some("x".repeat(2000)),
            duration_secs: Some(3600),
            created_at: chrono::Utc::now(),
            expires_at: None,
            dm_failed: true,
            removed_at: None,
            removed_by: None,
        }
    }

    #[test]
    fn embed_fields() {
        let v = serde_json::to_value(case_embed(&case(), CaseEvent::Created)).unwrap();
        assert_eq!(v["title"], "Case #7 · Timeout");
        let f = v["fields"].as_array().unwrap();
        let get = |n: &str| f.iter().find(|x| x["name"] == n).unwrap()["value"].as_str().unwrap().to_string();
        assert_eq!(get("Reason").chars().count(), 1024);
        assert_eq!(get("Moderator"), "Auto punishment");
        assert_eq!(get("Target"), "<@42> (42)");
        assert_eq!(get("Note"), "DM failed");
        assert!(f.iter().any(|x| x["name"] == "Duration"));
    }

    #[test]
    fn no_reason_default() {
        let mut c = case();
        c.reason = None;
        let v = serde_json::to_value(case_embed(&c, CaseEvent::Removed { by: 5 })).unwrap();
        assert!(v["fields"].as_array().unwrap().iter().any(|x| x["value"] == "No reason given"));
    }
}
