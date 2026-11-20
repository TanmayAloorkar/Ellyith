use crate::duration::MAX_TIMEOUT_SECS;
use crate::UserError;
use sqlx::{PgPool, Row};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LadderAction {
    Timeout,
    Kick,
    Tempban,
    Ban,
}

impl LadderAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Kick => "kick",
            Self::Tempban => "tempban",
            Self::Ban => "ban",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "timeout" => Self::Timeout,
            "kick" => Self::Kick,
            "tempban" => Self::Tempban,
            "ban" => Self::Ban,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LadderRule {
    pub warn_count: i32,
    pub action: LadderAction,
    pub duration_secs: Option<i64>,
}

pub fn default_rules() -> Vec<LadderRule> {
    vec![
        LadderRule { warn_count: 3, action: LadderAction::Timeout, duration_secs: Some(3600) },
        LadderRule { warn_count: 5, action: LadderAction::Kick, duration_secs: None },
        LadderRule { warn_count: 7, action: LadderAction::Ban, duration_secs: None },
    ]
}

/// A rule fires only when the count is reached exactly.
pub fn rule_for_count(rules: &[LadderRule], active_warns: i64) -> Option<&LadderRule> {
    rules.iter().find(|r| i64::from(r.warn_count) == active_warns)
}

pub fn validate_rule(r: &LadderRule) -> Result<(), UserError> {
    let bad = |m: &str| Err(UserError(m.into()));
    if r.warn_count < 1 {
        return bad("Warn count must be at least 1.");
    }
    match (r.action, r.duration_secs) {
        (LadderAction::Timeout, Some(d)) if (1..=MAX_TIMEOUT_SECS).contains(&d) => Ok(()),
        (LadderAction::Timeout, _) => bad("Timeout needs a duration of up to 28 days."),
        (LadderAction::Tempban, Some(d)) if d >= 1 => Ok(()),
        (LadderAction::Tempban, _) => bad("Tempban needs a duration."),
        (LadderAction::Kick | LadderAction::Ban, None) => Ok(()),
        (LadderAction::Kick | LadderAction::Ban, Some(_)) => bad("Kick and ban take no duration."),
    }
}

pub async fn list_rules(db: &PgPool, guild_id: i64) -> Result<Vec<LadderRule>, sqlx::Error> {
    let rows = sqlx::query("SELECT warn_count, action, duration_secs FROM ladder_rules WHERE guild_id = $1 ORDER BY warn_count")
        .bind(guild_id)
        .fetch_all(db)
        .await?;
    rows.iter()
        .map(|r| {
            let a: String = r.try_get("action")?;
            Ok(LadderRule {
                warn_count: r.try_get("warn_count")?,
                action: LadderAction::parse(&a).ok_or_else(|| sqlx::Error::Decode(format!("bad ladder action {a}").into()))?,
                duration_secs: r.try_get("duration_secs")?,
            })
        })
        .collect()
}

pub async fn set_rule(db: &PgPool, guild_id: i64, r: &LadderRule) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO ladder_rules (guild_id, warn_count, action, duration_secs) VALUES ($1, $2, $3, $4) \
         ON CONFLICT (guild_id, warn_count) DO UPDATE SET action = EXCLUDED.action, duration_secs = EXCLUDED.duration_secs",
    )
    .bind(guild_id)
    .bind(r.warn_count)
    .bind(r.action.as_str())
    .bind(r.duration_secs)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn remove_rule(db: &PgPool, guild_id: i64, warn_count: i32) -> Result<bool, sqlx::Error> {
    let n = sqlx::query("DELETE FROM ladder_rules WHERE guild_id = $1 AND warn_count = $2")
        .bind(guild_id)
        .bind(warn_count)
        .execute(db)
        .await?
        .rows_affected();
    Ok(n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::duration::MAX_TIMEOUT_SECS;
    fn r(warn_count: i32, action: LadderAction, duration_secs: Option<i64>) -> LadderRule {
        LadderRule { warn_count, action, duration_secs }
    }
    #[test]
    fn default_ladder_fires_on_exact_counts() {
        let rules = default_rules();
        assert_eq!(rule_for_count(&rules, 3).map(|r| r.action), Some(LadderAction::Timeout));
        assert_eq!(rules[0].duration_secs, Some(3600));
        assert_eq!(rule_for_count(&rules, 4), None);
        assert_eq!(rule_for_count(&rules, 5).map(|r| r.action), Some(LadderAction::Kick));
        assert_eq!(rule_for_count(&rules, 7).map(|r| r.action), Some(LadderAction::Ban));
    }
    #[test]
    fn validation() {
        assert!(validate_rule(&r(2, LadderAction::Timeout, None)).is_err());
        assert!(validate_rule(&r(2, LadderAction::Timeout, Some(MAX_TIMEOUT_SECS + 1))).is_err());
        assert!(validate_rule(&r(2, LadderAction::Kick, Some(60))).is_err());
        assert!(validate_rule(&r(0, LadderAction::Ban, None)).is_err());
        assert!(validate_rule(&r(2, LadderAction::Tempban, Some(86400))).is_ok());
    }
}
