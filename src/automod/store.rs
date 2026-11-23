use super::rules::{RuleAction, RuleConfig, RuleKind, WordEntry, WordKind};
use sqlx::{PgPool, Row};

pub async fn get_rules(db: &PgPool, guild: i64) -> Result<Vec<RuleConfig>, sqlx::Error> {
    let rows = sqlx::query("SELECT kind, enabled, action, timeout_secs, threshold, window_secs, percent, extra, ignored_roles, ignored_channels FROM automod_rules WHERE guild_id = $1")
        .bind(guild)
        .fetch_all(db)
        .await?;
    let mut saved = Vec::new();
    for r in rows {
        // Rows with an unknown kind/action fall back to defaults.
        let (Ok(kind), Ok(action)) = (r.get::<String, _>("kind").parse::<RuleKind>(), r.get::<String, _>("action").parse::<RuleAction>()) else { continue };
        saved.push(RuleConfig {
            kind,
            action,
            enabled: r.get("enabled"),
            timeout_secs: r.get("timeout_secs"),
            threshold: r.get("threshold"),
            window_secs: r.get("window_secs"),
            percent: r.get("percent"),
            extra: r.get("extra"),
            ignored_roles: r.get("ignored_roles"),
            ignored_channels: r.get("ignored_channels"),
        });
    }
    Ok(RuleKind::ALL.iter().map(|k| saved.iter().find(|c| c.kind == *k).cloned().unwrap_or_else(|| k.defaults())).collect())
}

pub async fn save_rule(db: &PgPool, guild: i64, c: &RuleConfig) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO automod_rules (guild_id, kind, enabled, action, timeout_secs, threshold, window_secs, percent, extra, ignored_roles, ignored_channels) \
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT (guild_id, kind) DO UPDATE SET enabled=$3, action=$4, timeout_secs=$5, \
         threshold=$6, window_secs=$7, percent=$8, extra=$9, ignored_roles=$10, ignored_channels=$11",
    )
    .bind(guild)
    .bind(c.kind.as_str())
    .bind(c.enabled)
    .bind(c.action.as_str())
    .bind(c.timeout_secs)
    .bind(c.threshold)
    .bind(c.window_secs)
    .bind(c.percent)
    .bind(c.extra)
    .bind(&c.ignored_roles)
    .bind(&c.ignored_channels)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn list_words(db: &PgPool, guild: i64) -> Result<Vec<WordEntry>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as("SELECT kind, pattern FROM automod_words WHERE guild_id = $1 ORDER BY kind, pattern").bind(guild).fetch_all(db).await?;
    Ok(rows.into_iter().filter_map(|(k, pattern)| Some(WordEntry { kind: k.parse::<WordKind>().ok()?, pattern })).collect())
}

pub async fn add_words(db: &PgPool, guild: i64, words: &[WordEntry]) -> Result<usize, sqlx::Error> {
    let mut n = 0;
    for w in words {
        n += sqlx::query("INSERT INTO automod_words (guild_id, kind, pattern) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
            .bind(guild)
            .bind(w.kind.as_str())
            .bind(&w.pattern)
            .execute(db)
            .await?
            .rows_affected() as usize;
    }
    Ok(n)
}

pub async fn remove_words(db: &PgPool, guild: i64, words: &[WordEntry]) -> Result<usize, sqlx::Error> {
    let mut n = 0;
    for w in words {
        n += sqlx::query("DELETE FROM automod_words WHERE guild_id=$1 AND kind=$2 AND pattern=$3")
            .bind(guild)
            .bind(w.kind.as_str())
            .bind(&w.pattern)
            .execute(db)
            .await?
            .rows_affected() as usize;
    }
    Ok(n)
}

pub async fn list_sites(db: &PgPool, guild: i64) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT site FROM automod_sites WHERE guild_id = $1 ORDER BY site").bind(guild).fetch_all(db).await
}

pub async fn add_site(db: &PgPool, guild: i64, site: &str) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("INSERT INTO automod_sites (guild_id, site) VALUES ($1,$2) ON CONFLICT DO NOTHING").bind(guild).bind(site).execute(db).await?.rows_affected() > 0)
}

pub async fn remove_site(db: &PgPool, guild: i64, site: &str) -> Result<bool, sqlx::Error> {
    Ok(sqlx::query("DELETE FROM automod_sites WHERE guild_id=$1 AND site=$2").bind(guild).bind(site).execute(db).await?.rows_affected() > 0)
}

/// Replaces the whole stored list.
pub async fn save_scam_domains(db: &PgPool, domains: &[String]) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM automod_scam_domains").execute(&mut *tx).await?;
    sqlx::query("INSERT INTO automod_scam_domains (domain) SELECT DISTINCT unnest($1::text[])").bind(domains).execute(&mut *tx).await?;
    tx.commit().await
}

pub async fn load_scam_domains(db: &PgPool) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT domain FROM automod_scam_domains ORDER BY domain").fetch_all(db).await
}
