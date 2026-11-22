use chrono::{DateTime, Utc};
use sqlx::postgres::PgRow;
use sqlx::{PgPool, Row};
use std::str::FromStr;

macro_rules! case_kinds {
    ($($v:ident => $s:literal, $label:literal;)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum CaseKind { $($v),* }
        impl CaseKind {
            pub fn as_str(self) -> &'static str { match self { $(Self::$v => $s),* } }
            pub fn label(&self) -> &'static str { match self { $(Self::$v => $label),* } }
        }
        impl FromStr for CaseKind {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, String> {
                match s { $($s => Ok(Self::$v),)* _ => Err(format!("unknown case kind {s}")) }
            }
        }
    };
}

case_kinds! {
    Warn => "warn", "Warning";
    Timeout => "timeout", "Timeout";
    Untimeout => "untimeout", "Timeout removed";
    Kick => "kick", "Kick";
    Ban => "ban", "Ban";
    Tempban => "tempban", "Temporary ban";
    Unban => "unban", "Unban";
    NickSet => "nick_set", "Nickname set";
    NickReset => "nick_reset", "Nickname reset";
    VoiceDisconnect => "voice_disconnect", "Voice disconnect";
    VoiceMute => "voice_mute", "Voice mute";
    VoiceUnmute => "voice_unmute", "Voice unmute";
    VoiceMove => "voice_move", "Voice move";
    Quarantine => "quarantine", "Quarantine";
    QuarantineRelease => "quarantine_release", "Quarantine release";
}

impl CaseKind {
    /// Only punishments create cases; everything else is a log entry.
    pub fn is_punishment(self) -> bool {
        matches!(self, Self::Warn | Self::Timeout | Self::Kick | Self::Ban | Self::Tempban | Self::VoiceMute)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    Moderator(i64),
    AutoPunishment,
    System,
}

impl Actor {
    fn columns(self) -> (&'static str, Option<i64>) {
        match self {
            Self::Moderator(id) => ("moderator", Some(id)),
            Self::AutoPunishment => ("auto_punishment", None),
            Self::System => ("system", None),
        }
    }
    fn from_columns(actor: &str, moderator_id: Option<i64>) -> Option<Self> {
        match (actor, moderator_id) {
            ("moderator", Some(id)) => Some(Self::Moderator(id)),
            ("auto_punishment", None) => Some(Self::AutoPunishment),
            ("system", None) => Some(Self::System),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewCase {
    pub guild_id: i64,
    pub kind: CaseKind,
    pub target_id: i64,
    pub actor: Actor,
    pub reason: Option<String>,
    pub duration_secs: Option<i64>,
    pub expires_at: Option<DateTime<Utc>>,
    pub dm_failed: bool,
}

#[derive(Debug, Clone)]
pub struct Case {
    pub guild_id: i64,
    pub case_number: i32,
    pub kind: CaseKind,
    pub target_id: i64,
    pub actor: Actor,
    pub reason: Option<String>,
    pub duration_secs: Option<i64>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub dm_failed: bool,
    pub removed_at: Option<DateTime<Utc>>,
    pub removed_by: Option<i64>,
}

fn decode(e: String) -> sqlx::Error {
    sqlx::Error::Decode(e.into())
}

fn from_row(r: &PgRow) -> Result<Case, sqlx::Error> {
    let kind: String = r.try_get("kind")?;
    let actor: String = r.try_get("actor")?;
    Ok(Case {
        guild_id: r.try_get("guild_id")?,
        case_number: r.try_get("case_number")?,
        kind: kind.parse().map_err(decode)?,
        target_id: r.try_get("target_id")?,
        actor: Actor::from_columns(&actor, r.try_get("moderator_id")?).ok_or_else(|| decode(format!("bad actor {actor}")))?,
        reason: r.try_get("reason")?,
        duration_secs: r.try_get("duration_secs")?,
        created_at: r.try_get("created_at")?,
        expires_at: r.try_get("expires_at")?,
        dm_failed: r.try_get("dm_failed")?,
        removed_at: r.try_get("removed_at")?,
        removed_by: r.try_get("removed_by")?,
    })
}

pub async fn reserve_number(db: &PgPool, guild_id: i64) -> Result<i32, sqlx::Error> {
    sqlx::query_scalar(
        "INSERT INTO case_counters (guild_id, next) VALUES ($1, 2) \
         ON CONFLICT (guild_id) DO UPDATE SET next = case_counters.next + 1 RETURNING next - 1",
    )
    .bind(guild_id)
    .fetch_one(db)
    .await
}

pub async fn insert(db: &PgPool, number: i32, c: &NewCase) -> Result<Case, sqlx::Error> {
    let (actor, moderator_id) = c.actor.columns();
    let row = sqlx::query(
        "INSERT INTO cases (guild_id, case_number, kind, target_id, actor, moderator_id, reason, duration_secs, expires_at, dm_failed) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING *",
    )
    .bind(c.guild_id)
    .bind(number)
    .bind(c.kind.as_str())
    .bind(c.target_id)
    .bind(actor)
    .bind(moderator_id)
    .bind(&c.reason)
    .bind(c.duration_secs)
    .bind(c.expires_at)
    .bind(c.dm_failed)
    .fetch_one(db)
    .await?;
    from_row(&row)
}

pub async fn create(db: &PgPool, c: &NewCase) -> Result<Case, sqlx::Error> {
    let n = reserve_number(db, c.guild_id).await?;
    insert(db, n, c).await
}

pub async fn get(db: &PgPool, guild_id: i64, number: i32) -> Result<Option<Case>, sqlx::Error> {
    let row = sqlx::query("SELECT * FROM cases WHERE guild_id = $1 AND case_number = $2")
        .bind(guild_id)
        .bind(number)
        .fetch_optional(db)
        .await?;
    row.as_ref().map(from_row).transpose()
}

pub async fn set_reason(db: &PgPool, guild_id: i64, number: i32, reason: Option<String>) -> Result<Option<Case>, sqlx::Error> {
    let row = sqlx::query("UPDATE cases SET reason = $3 WHERE guild_id = $1 AND case_number = $2 RETURNING *")
        .bind(guild_id)
        .bind(number)
        .bind(reason)
        .fetch_optional(db)
        .await?;
    row.as_ref().map(from_row).transpose()
}

/// None if the case is missing or already removed.
pub async fn soft_remove(db: &PgPool, guild_id: i64, number: i32, by: i64) -> Result<Option<Case>, sqlx::Error> {
    let row = sqlx::query(
        "UPDATE cases SET removed_at = now(), removed_by = $3 \
         WHERE guild_id = $1 AND case_number = $2 AND removed_at IS NULL RETURNING *",
    )
    .bind(guild_id)
    .bind(number)
    .bind(by)
    .fetch_optional(db)
    .await?;
    row.as_ref().map(from_row).transpose()
}

pub async fn history(db: &PgPool, guild_id: i64, target_id: i64, include_removed: bool) -> Result<Vec<Case>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT * FROM cases WHERE guild_id = $1 AND target_id = $2 AND ($3 OR removed_at IS NULL) ORDER BY case_number DESC",
    )
    .bind(guild_id)
    .bind(target_id)
    .bind(include_removed)
    .fetch_all(db)
    .await?;
    rows.iter().map(from_row).collect()
}

/// Position of a case among the target's cases (removed included).
pub async fn ordinal(db: &PgPool, guild_id: i64, target_id: i64, case_number: i32) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("SELECT count(*) FROM cases WHERE guild_id = $1 AND target_id = $2 AND case_number <= $3")
        .bind(guild_id)
        .bind(target_id)
        .bind(case_number)
        .fetch_one(db)
        .await
}

pub async fn active_warn_count(db: &PgPool, guild_id: i64, target_id: i64, decay_days: Option<i32>) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT count(*) FROM cases WHERE guild_id = $1 AND target_id = $2 AND kind = 'warn' AND removed_at IS NULL \
         AND ($3::int IS NULL OR created_at > now() - make_interval(days => $3))",
    )
    .bind(guild_id)
    .bind(target_id)
    .bind(decay_days)
    .fetch_one(db)
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punishment_kinds() {
        for k in [CaseKind::Warn, CaseKind::Timeout, CaseKind::Kick, CaseKind::Ban, CaseKind::Tempban, CaseKind::VoiceMute] {
            assert!(k.is_punishment());
        }
        for k in [CaseKind::Untimeout, CaseKind::Unban, CaseKind::NickSet, CaseKind::NickReset, CaseKind::VoiceUnmute, CaseKind::VoiceDisconnect, CaseKind::VoiceMove] {
            assert!(!k.is_punishment());
        }
        assert!(!CaseKind::Quarantine.is_punishment());
        assert_eq!("quarantine_release".parse::<CaseKind>(), Ok(CaseKind::QuarantineRelease));
    }
}
