use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct SafetySettings {
    pub guild_id: i64,
    pub quarantine_role_id: Option<i64>,
    pub review_channel_id: Option<i64>,
    pub deny_action: String,
    pub verify_mode: String,
    pub verify_review: String,
    pub verify_channel_id: Option<i64>,
    pub verify_roles: Vec<i64>,
    pub verify_description: String,
    pub verify_message_id: Option<i64>,
    pub quiz_questions: Vec<String>,
    pub quiz_answers: Vec<String>,
    pub lockdown_channels: Vec<i64>,
    pub lockdown_message: String,
}

pub async fn get_settings(db: &PgPool, guild_id: i64) -> Result<SafetySettings, sqlx::Error> {
    sqlx::query("INSERT INTO safety_settings (guild_id) VALUES ($1) ON CONFLICT DO NOTHING").bind(guild_id).execute(db).await?;
    sqlx::query_as("SELECT * FROM safety_settings WHERE guild_id = $1").bind(guild_id).fetch_one(db).await
}

pub async fn save_settings(db: &PgPool, s: &SafetySettings) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE safety_settings SET quarantine_role_id=$2, review_channel_id=$3, deny_action=$4, verify_mode=$5, \
         verify_review=$6, verify_channel_id=$7, verify_roles=$8, verify_description=$9, verify_message_id=$10, \
         quiz_questions=$11, quiz_answers=$12, lockdown_channels=$13, lockdown_message=$14 WHERE guild_id=$1",
    )
    .bind(s.guild_id)
    .bind(s.quarantine_role_id)
    .bind(s.review_channel_id)
    .bind(&s.deny_action)
    .bind(&s.verify_mode)
    .bind(&s.verify_review)
    .bind(s.verify_channel_id)
    .bind(&s.verify_roles)
    .bind(&s.verify_description)
    .bind(s.verify_message_id)
    .bind(&s.quiz_questions)
    .bind(&s.quiz_answers)
    .bind(&s.lockdown_channels)
    .bind(&s.lockdown_message)
    .execute(db)
    .await?;
    Ok(())
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
pub struct QuarantineRow {
    pub guild_id: i64,
    pub user_id: i64,
    pub reason: String,
    pub saved_roles: Vec<i64>,
    pub answers: Option<Vec<String>>,
    pub created_at: DateTime<Utc>,
    pub review_channel_id: Option<i64>,
    pub review_message_id: Option<i64>,
}

/// False when the member is already quarantined. `row.created_at` is ignored; the DB default sets it.
pub async fn quarantine_insert(db: &PgPool, row: &QuarantineRow) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query(
        "INSERT INTO quarantines (guild_id, user_id, reason, saved_roles, answers, review_channel_id, review_message_id) \
         VALUES ($1,$2,$3,$4,$5,$6,$7) ON CONFLICT DO NOTHING",
    )
    .bind(row.guild_id)
    .bind(row.user_id)
    .bind(&row.reason)
    .bind(&row.saved_roles)
    .bind(&row.answers)
    .bind(row.review_channel_id)
    .bind(row.review_message_id)
    .execute(db)
    .await?
    .rows_affected()
        == 1)
}

pub async fn quarantine_get(db: &PgPool, guild_id: i64, user_id: i64) -> Result<Option<QuarantineRow>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM quarantines WHERE guild_id=$1 AND user_id=$2").bind(guild_id).bind(user_id).fetch_optional(db).await
}

pub async fn quarantine_delete(db: &PgPool, guild_id: i64, user_id: i64) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM quarantines WHERE guild_id=$1 AND user_id=$2").bind(guild_id).bind(user_id).execute(db).await?.rows_affected() == 1)
}

pub async fn quarantine_list(db: &PgPool, guild_id: i64) -> Result<Vec<QuarantineRow>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM quarantines WHERE guild_id=$1 ORDER BY created_at, user_id").bind(guild_id).fetch_all(db).await
}

pub async fn quarantine_set_review(db: &PgPool, guild_id: i64, user_id: i64, channel_id: i64, message_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE quarantines SET review_channel_id=$3, review_message_id=$4 WHERE guild_id=$1 AND user_id=$2")
        .bind(guild_id)
        .bind(user_id)
        .bind(channel_id)
        .bind(message_id)
        .execute(db)
        .await?;
    Ok(())
}

/// False when there is no row to update.
pub async fn quarantine_set_answers(db: &PgPool, guild_id: i64, user_id: i64, answers: &[String]) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("UPDATE quarantines SET answers=$3 WHERE guild_id=$1 AND user_id=$2").bind(guild_id).bind(user_id).bind(answers).execute(db).await?.rows_affected() == 1)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedOverwrite {
    pub target_id: i64,
    pub is_member: bool,
    pub allow: i64,
    pub deny: i64,
}

pub async fn lockdown_save_channel(db: &PgPool, guild_id: i64, channel_id: i64, saved: &[SavedOverwrite], posted_message: bool) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query("INSERT INTO lockdown_channels (guild_id, channel_id, posted_message) VALUES ($1,$2,$3) ON CONFLICT (guild_id, channel_id) DO UPDATE SET posted_message = EXCLUDED.posted_message")
        .bind(guild_id)
        .bind(channel_id)
        .bind(posted_message)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM lockdown_overwrites WHERE guild_id=$1 AND channel_id=$2").bind(guild_id).bind(channel_id).execute(&mut *tx).await?;
    for o in saved {
        sqlx::query("INSERT INTO lockdown_overwrites (guild_id, channel_id, target_id, is_member, allow, deny) VALUES ($1,$2,$3,$4,$5,$6)")
            .bind(guild_id)
            .bind(channel_id)
            .bind(o.target_id)
            .bind(o.is_member)
            .bind(o.allow)
            .bind(o.deny)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await
}

pub async fn lockdown_load(db: &PgPool, guild_id: i64) -> Result<Vec<(i64, bool, Vec<SavedOverwrite>)>, sqlx::Error> {
    let chans: Vec<(i64, bool)> = sqlx::query_as("SELECT channel_id, posted_message FROM lockdown_channels WHERE guild_id=$1 ORDER BY channel_id").bind(guild_id).fetch_all(db).await?;
    let ows: Vec<(i64, i64, bool, i64, i64)> =
        sqlx::query_as("SELECT channel_id, target_id, is_member, allow, deny FROM lockdown_overwrites WHERE guild_id=$1 ORDER BY target_id, is_member").bind(guild_id).fetch_all(db).await?;
    Ok(chans
        .into_iter()
        .map(|(c, posted)| {
            let v = ows.iter().filter(|o| o.0 == c).map(|o| SavedOverwrite { target_id: o.1, is_member: o.2, allow: o.3, deny: o.4 }).collect();
            (c, posted, v)
        })
        .collect())
}

pub async fn lockdown_active(db: &PgPool, guild_id: i64) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM lockdown_channels WHERE guild_id=$1)").bind(guild_id).fetch_one(db).await
}

pub async fn lockdown_clear(db: &PgPool, guild_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM lockdown_channels WHERE guild_id=$1").bind(guild_id).execute(db).await?;
    Ok(())
}
