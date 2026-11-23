use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct ProtectionSettings {
    pub guild_id: i64,
    pub raid_enabled: bool,
    pub raid_joins: i32,
    pub raid_secs: i32,
    pub raid_responses: Vec<String>,
    pub raid_filter: Vec<String>,
    pub raid_filter_days: i32,
    pub raid_end_mins: i32,
    pub alt_enabled: bool,
    pub alt_days: i32,
    pub alt_pts_age: i32,
    pub alt_pts_avatar: i32,
    pub alt_pts_name: i32,
    pub alt_pts_invite: i32,
    pub alt_threshold: i32,
    pub alt_response: String,
    pub gate_enabled: bool,
    pub gate_days: i32,
    pub gate_action: String,
    pub nuke_enabled: bool,
    pub nuke_window_secs: i32,
    pub nuke_bans: i32,
    pub nuke_kicks: i32,
    pub nuke_chan_del: i32,
    pub nuke_role_del: i32,
    pub nuke_webhooks: i32,
    pub nuke_perms: i32,
}

/// A missing row is created with the column defaults.
pub async fn get_settings(db: &PgPool, guild: i64) -> Result<ProtectionSettings, sqlx::Error> {
    sqlx::query("INSERT INTO protection_settings (guild_id) VALUES ($1) ON CONFLICT DO NOTHING").bind(guild).execute(db).await?;
    sqlx::query_as("SELECT * FROM protection_settings WHERE guild_id = $1").bind(guild).fetch_one(db).await
}

pub async fn save_settings(db: &PgPool, s: &ProtectionSettings) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO protection_settings (guild_id, raid_enabled, raid_joins, raid_secs, raid_responses, raid_filter, raid_filter_days, raid_end_mins, \
         alt_enabled, alt_days, alt_pts_age, alt_pts_avatar, alt_pts_name, alt_pts_invite, alt_threshold, alt_response, \
         gate_enabled, gate_days, gate_action, nuke_enabled, nuke_window_secs, nuke_bans, nuke_kicks, nuke_chan_del, nuke_role_del, nuke_webhooks, nuke_perms) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21,$22,$23,$24,$25,$26,$27) \
         ON CONFLICT (guild_id) DO UPDATE SET raid_enabled=$2, raid_joins=$3, raid_secs=$4, raid_responses=$5, raid_filter=$6, raid_filter_days=$7, raid_end_mins=$8, \
         alt_enabled=$9, alt_days=$10, alt_pts_age=$11, alt_pts_avatar=$12, alt_pts_name=$13, alt_pts_invite=$14, alt_threshold=$15, alt_response=$16, \
         gate_enabled=$17, gate_days=$18, gate_action=$19, nuke_enabled=$20, nuke_window_secs=$21, nuke_bans=$22, nuke_kicks=$23, nuke_chan_del=$24, \
         nuke_role_del=$25, nuke_webhooks=$26, nuke_perms=$27",
    )
    .bind(s.guild_id)
    .bind(s.raid_enabled)
    .bind(s.raid_joins)
    .bind(s.raid_secs)
    .bind(&s.raid_responses)
    .bind(&s.raid_filter)
    .bind(s.raid_filter_days)
    .bind(s.raid_end_mins)
    .bind(s.alt_enabled)
    .bind(s.alt_days)
    .bind(s.alt_pts_age)
    .bind(s.alt_pts_avatar)
    .bind(s.alt_pts_name)
    .bind(s.alt_pts_invite)
    .bind(s.alt_threshold)
    .bind(&s.alt_response)
    .bind(s.gate_enabled)
    .bind(s.gate_days)
    .bind(&s.gate_action)
    .bind(s.nuke_enabled)
    .bind(s.nuke_window_secs)
    .bind(s.nuke_bans)
    .bind(s.nuke_kicks)
    .bind(s.nuke_chan_del)
    .bind(s.nuke_role_del)
    .bind(s.nuke_webhooks)
    .bind(s.nuke_perms)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct RaidRow {
    pub id: i64,
    pub guild_id: i64,
    pub started_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub alert_channel: Option<i64>,
    pub alert_message: Option<i64>,
    pub started_lockdown: bool,
}

const RAID_COLS: &str = "id, guild_id, started_at, ends_at, alert_channel, alert_message, started_lockdown";

pub async fn active_raid(db: &PgPool, guild: i64) -> Result<Option<RaidRow>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {RAID_COLS} FROM protection_raids WHERE guild_id=$1 AND ended_at IS NULL")).bind(guild).fetch_optional(db).await
}

/// The unique index allows one active raid per guild; a second start errors.
pub async fn start_raid(db: &PgPool, guild: i64, ends_at: DateTime<Utc>) -> Result<RaidRow, sqlx::Error> {
    sqlx::query_as(&format!("INSERT INTO protection_raids (guild_id, ends_at) VALUES ($1,$2) RETURNING {RAID_COLS}")).bind(guild).bind(ends_at).fetch_one(db).await
}

pub async fn extend_raid(db: &PgPool, id: i64, ends_at: DateTime<Utc>) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE protection_raids SET ends_at=$2 WHERE id=$1").bind(id).bind(ends_at).execute(db).await?;
    Ok(())
}

pub async fn set_raid_alert(db: &PgPool, id: i64, channel: i64, message: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE protection_raids SET alert_channel=$2, alert_message=$3 WHERE id=$1").bind(id).bind(channel).bind(message).execute(db).await?;
    Ok(())
}

pub async fn set_raid_lockdown(db: &PgPool, id: i64, started: bool) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE protection_raids SET started_lockdown=$2 WHERE id=$1").bind(id).bind(started).execute(db).await?;
    Ok(())
}

/// False when the raid had already ended.
pub async fn end_raid(db: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("UPDATE protection_raids SET ended_at=now() WHERE id=$1 AND ended_at IS NULL").bind(id).execute(db).await?.rows_affected() == 1)
}

/// When the guild's last raid ended; `None` if it never had one.
pub async fn last_raid_end(db: &PgPool, guild: i64) -> Result<Option<DateTime<Utc>>, sqlx::Error> {
    Ok(sqlx::query_as::<_, (Option<DateTime<Utc>>,)>("SELECT max(ended_at) FROM protection_raids WHERE guild_id=$1").bind(guild).fetch_one(db).await?.0)
}

/// Active raids whose end time has passed.
pub async fn due_raids(db: &PgPool, now: DateTime<Utc>) -> Result<Vec<RaidRow>, sqlx::Error> {
    sqlx::query_as(&format!("SELECT {RAID_COLS} FROM protection_raids WHERE ended_at IS NULL AND ends_at <= $1")).bind(now).fetch_all(db).await
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct RaiderRow {
    pub raid_id: i64,
    pub user_id: i64,
    pub account_created: DateTime<Utc>,
    pub default_avatar: bool,
}

pub async fn add_raider(db: &PgPool, raid_id: i64, user: i64, account_created: DateTime<Utc>, default_avatar: bool) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO protection_raiders (raid_id, user_id, account_created, default_avatar) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
        .bind(raid_id)
        .bind(user)
        .bind(account_created)
        .bind(default_avatar)
        .execute(db)
        .await?;
    Ok(())
}

pub async fn raiders(db: &PgPool, raid_id: i64) -> Result<Vec<RaiderRow>, sqlx::Error> {
    sqlx::query_as("SELECT raid_id, user_id, account_created, default_avatar FROM protection_raiders WHERE raid_id=$1 ORDER BY user_id").bind(raid_id).fetch_all(db).await
}

pub async fn record_member_invite(db: &PgPool, guild: i64, user: i64, code: Option<&str>, inviter: Option<i64>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO protection_invites (guild_id, user_id, invite_code, inviter_id) VALUES ($1,$2,$3,$4) \
         ON CONFLICT (guild_id, user_id) DO UPDATE SET invite_code=$3, inviter_id=$4",
    )
    .bind(guild)
    .bind(user)
    .bind(code)
    .bind(inviter)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn member_invite(db: &PgPool, guild: i64, user: i64) -> Result<Option<(Option<String>, Option<i64>)>, sqlx::Error> {
    sqlx::query_as("SELECT invite_code, inviter_id FROM protection_invites WHERE guild_id=$1 AND user_id=$2").bind(guild).bind(user).fetch_optional(db).await
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct BanRow {
    pub guild_id: i64,
    pub user_id: i64,
    pub username: String,
    pub display_name: String,
    pub invite_code: Option<String>,
    pub banned_at: DateTime<Utc>,
}

pub async fn record_ban(db: &PgPool, guild: i64, user: i64, username: &str, display_name: &str, invite_code: Option<&str>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO protection_bans (guild_id, user_id, username, display_name, invite_code) VALUES ($1,$2,$3,$4,$5) \
         ON CONFLICT (guild_id, user_id) DO UPDATE SET username=$3, display_name=$4, invite_code=$5, banned_at=now()",
    )
    .bind(guild)
    .bind(user)
    .bind(username)
    .bind(display_name)
    .bind(invite_code)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn recent_bans(db: &PgPool, guild: i64, since: DateTime<Utc>) -> Result<Vec<BanRow>, sqlx::Error> {
    sqlx::query_as("SELECT guild_id, user_id, username, display_name, invite_code, banned_at FROM protection_bans WHERE guild_id=$1 AND banned_at >= $2 ORDER BY banned_at DESC")
        .bind(guild)
        .bind(since)
        .fetch_all(db)
        .await
}

pub async fn prune_bans(db: &PgPool, before: DateTime<Utc>) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM protection_bans WHERE banned_at < $1").bind(before).execute(db).await?.rows_affected())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrustKind {
    User,
    Role,
}

impl TrustKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Role => "role",
        }
    }
}

pub async fn trusted(db: &PgPool, guild: i64) -> Result<Vec<(TrustKind, i64)>, sqlx::Error> {
    let rows: Vec<(String, i64)> = sqlx::query_as("SELECT kind, target_id FROM protection_trusted WHERE guild_id=$1 ORDER BY kind, target_id").bind(guild).fetch_all(db).await?;
    Ok(rows.into_iter().map(|(k, id)| (if k == "role" { TrustKind::Role } else { TrustKind::User }, id)).collect())
}

pub async fn trust_add(db: &PgPool, guild: i64, kind: TrustKind, id: i64) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("INSERT INTO protection_trusted (guild_id, kind, target_id) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING").bind(guild).bind(kind.as_str()).bind(id).execute(db).await?.rows_affected() > 0)
}

pub async fn trust_remove(db: &PgPool, guild: i64, kind: TrustKind, id: i64) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM protection_trusted WHERE guild_id=$1 AND kind=$2 AND target_id=$3").bind(guild).bind(kind.as_str()).bind(id).execute(db).await?.rows_affected() > 0)
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct StripRow {
    pub id: i64,
    pub guild_id: i64,
    pub user_id: i64,
    pub roles: Vec<i64>,
    pub reason: String,
    pub created_at: DateTime<Utc>,
    pub restored: bool,
}

pub async fn record_strip(db: &PgPool, guild: i64, user: i64, roles: &[i64], reason: &str) -> Result<i64, sqlx::Error> {
    sqlx::query_scalar("INSERT INTO protection_strips (guild_id, user_id, roles, reason) VALUES ($1,$2,$3,$4) RETURNING id").bind(guild).bind(user).bind(roles).bind(reason).fetch_one(db).await
}

pub async fn get_strip(db: &PgPool, id: i64) -> Result<Option<StripRow>, sqlx::Error> {
    sqlx::query_as("SELECT id, guild_id, user_id, roles, reason, created_at, restored FROM protection_strips WHERE id=$1").bind(id).fetch_optional(db).await
}

/// False when the strip was already restored (or does not exist).
pub async fn mark_strip_restored(db: &PgPool, id: i64) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("UPDATE protection_strips SET restored=TRUE WHERE id=$1 AND NOT restored").bind(id).execute(db).await?.rows_affected() > 0)
}

/// Undo a claim made by `mark_strip_restored` when giving the roles back failed.
pub async fn unmark_strip_restored(db: &PgPool, id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE protection_strips SET restored=FALSE WHERE id=$1").bind(id).execute(db).await?;
    Ok(())
}
