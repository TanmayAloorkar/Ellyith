use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    Unban,
}

impl JobKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unban => "unban",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Job {
    pub id: i64,
    pub guild_id: i64,
    pub kind: JobKind,
    pub target_id: i64,
    pub run_at: DateTime<Utc>,
}

#[derive(FromRow)]
struct Row {
    id: i64,
    guild_id: i64,
    kind: String,
    target_id: i64,
    run_at: DateTime<Utc>,
}

pub async fn schedule(db: &PgPool, guild_id: i64, kind: JobKind, target_id: i64, run_at: DateTime<Utc>) -> sqlx::Result<i64> {
    sqlx::query_scalar("INSERT INTO scheduled_jobs (guild_id, kind, target_id, run_at) VALUES ($1, $2, $3, $4) RETURNING id")
        .bind(guild_id)
        .bind(kind.as_str())
        .bind(target_id)
        .bind(run_at)
        .fetch_one(db)
        .await
}

pub async fn due(db: &PgPool, now: DateTime<Utc>) -> sqlx::Result<Vec<Job>> {
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT id, guild_id, kind, target_id, run_at FROM scheduled_jobs WHERE NOT done AND run_at <= $1 ORDER BY run_at LIMIT 100",
    )
    .bind(now)
    .fetch_all(db)
    .await?;
    // Only 'unban' exists today; unknown kinds are skipped rather than mis-run.
    Ok(rows
        .into_iter()
        .filter(|r| r.kind == JobKind::Unban.as_str())
        .map(|r| Job { id: r.id, guild_id: r.guild_id, kind: JobKind::Unban, target_id: r.target_id, run_at: r.run_at })
        .collect())
}

pub async fn mark_done(db: &PgPool, id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE scheduled_jobs SET done = true WHERE id = $1").bind(id).execute(db).await?;
    Ok(())
}

/// Marks pending jobs for the target done; returns rows affected.
pub async fn cancel(db: &PgPool, guild_id: i64, kind: JobKind, target_id: i64) -> sqlx::Result<u64> {
    let r = sqlx::query("UPDATE scheduled_jobs SET done = true WHERE guild_id = $1 AND kind = $2 AND target_id = $3 AND NOT done")
        .bind(guild_id)
        .bind(kind.as_str())
        .bind(target_id)
        .execute(db)
        .await?;
    Ok(r.rows_affected())
}
