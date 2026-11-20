use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Note {
    pub id: i64,
    pub guild_id: i64,
    pub target_id: i64,
    pub author_id: i64,
    pub content: String,
    pub created_at: DateTime<Utc>,
}

pub async fn add(db: &PgPool, guild_id: i64, target_id: i64, author_id: i64, content: &str) -> Result<Note, sqlx::Error> {
    sqlx::query_as("INSERT INTO notes (guild_id, target_id, author_id, content) VALUES ($1, $2, $3, $4) RETURNING *")
        .bind(guild_id)
        .bind(target_id)
        .bind(author_id)
        .bind(content)
        .fetch_one(db)
        .await
}

/// Newest first.
pub async fn list(db: &PgPool, guild_id: i64, target_id: i64) -> Result<Vec<Note>, sqlx::Error> {
    sqlx::query_as("SELECT * FROM notes WHERE guild_id = $1 AND target_id = $2 ORDER BY id DESC")
        .bind(guild_id)
        .bind(target_id)
        .fetch_all(db)
        .await
}

/// Hard delete; false when no such note in this guild.
pub async fn remove(db: &PgPool, guild_id: i64, id: i64) -> Result<bool, sqlx::Error> {
    let r = sqlx::query("DELETE FROM notes WHERE guild_id = $1 AND id = $2").bind(guild_id).bind(id).execute(db).await?;
    Ok(r.rows_affected() > 0)
}
