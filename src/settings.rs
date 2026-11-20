use crate::moderation::ladder::default_rules;
use sqlx::PgPool;

#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct GuildSettings {
    pub guild_id: i64,
    pub mod_roles: Vec<i64>,
    pub protected_roles: Vec<i64>,
    pub dm_show_moderator: bool,
    pub warn_decay_enabled: bool,
    pub warn_decay_days: i32,
    pub filtered_words: Vec<String>,
    pub hoist_chars: String,
    pub nick_fallback: String,
    pub nick_filter_enabled: bool,
}

impl GuildSettings {
    /// Same values as the SQL column defaults in `0001_settings.sql`.
    pub fn defaults(guild_id: i64) -> Self {
        Self {
            guild_id,
            mod_roles: vec![],
            protected_roles: vec![],
            dm_show_moderator: false,
            warn_decay_enabled: false,
            warn_decay_days: 30,
            filtered_words: vec![],
            hoist_chars: "!".into(),
            nick_fallback: "Moderated Nickname".into(),
            nick_filter_enabled: true,
        }
    }
}

/// Fetch settings, creating the row (and seeding the default ladder, once) on first use.
pub async fn get_or_create(db: &PgPool, guild_id: i64) -> Result<GuildSettings, sqlx::Error> {
    let mut tx = db.begin().await?;
    let inserted = sqlx::query("INSERT INTO guild_settings (guild_id) VALUES ($1) ON CONFLICT DO NOTHING")
        .bind(guild_id)
        .execute(&mut *tx)
        .await?
        .rows_affected()
        == 1;
    if inserted {
        for r in default_rules() {
            sqlx::query("INSERT INTO ladder_rules (guild_id, warn_count, action, duration_secs) VALUES ($1, $2, $3, $4)")
                .bind(guild_id)
                .bind(r.warn_count)
                .bind(r.action.as_str())
                .bind(r.duration_secs)
                .execute(&mut *tx)
                .await?;
        }
    }
    let s = sqlx::query_as("SELECT * FROM guild_settings WHERE guild_id = $1")
        .bind(guild_id)
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(s)
}

pub async fn save(db: &PgPool, s: &GuildSettings) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE guild_settings SET mod_roles=$2, protected_roles=$3, dm_show_moderator=$4, \
         warn_decay_enabled=$5, warn_decay_days=$6, filtered_words=$7, hoist_chars=$8, \
         nick_fallback=$9, nick_filter_enabled=$10 WHERE guild_id=$1",
    )
    .bind(s.guild_id)
    .bind(&s.mod_roles)
    .bind(&s.protected_roles)
    .bind(s.dm_show_moderator)
    .bind(s.warn_decay_enabled)
    .bind(s.warn_decay_days)
    .bind(&s.filtered_words)
    .bind(&s.hoist_chars)
    .bind(&s.nick_fallback)
    .bind(s.nick_filter_enabled)
    .execute(db)
    .await?;
    Ok(())
}
