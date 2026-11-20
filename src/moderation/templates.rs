use crate::moderation::cases::CaseKind;
use crate::text::truncate;
use sqlx::PgPool;

pub struct TemplateVars<'a> {
    pub action: &'a str,
    pub reason: &'a str,
    pub duration: Option<&'a str>,
    pub server: &'a str,
    pub case_number: i32,
    pub moderator: Option<&'a str>,
}

pub fn dm_kinds() -> &'static [CaseKind] {
    use CaseKind::*;
    &[Warn, Timeout, Kick, Ban, Tempban]
}

pub fn default_template(kind: CaseKind) -> &'static str {
    match kind {
        CaseKind::Warn => "You were warned in **{server}**.\nReason: {reason}\nCase #{case}",
        CaseKind::Timeout => "You were timed out in **{server}** for {duration}.\nReason: {reason}\nCase #{case}",
        CaseKind::Kick => "You were kicked from **{server}**.\nReason: {reason}\nCase #{case}",
        CaseKind::Ban => "You were banned from **{server}**.\nReason: {reason}\nCase #{case}",
        CaseKind::Tempban => "You were banned from **{server}** for {duration}.\nReason: {reason}\nCase #{case}",
        _ => panic!("no DM template for {}", kind.as_str()),
    }
}

pub fn render(template: &str, v: &TemplateVars) -> String {
    // shortcut: user-controlled values (reason) are substituted last so they cannot inject other placeholders.
    let mut out = template
        .replace("{action}", v.action)
        .replace("{duration}", v.duration.unwrap_or(""))
        .replace("{server}", v.server)
        .replace("{case}", &v.case_number.to_string())
        .replace("{moderator}", v.moderator.unwrap_or("Staff"))
        .replace("{reason}", v.reason);
    if let Some(m) = v.moderator {
        if !template.contains("{moderator}") {
            out.push_str(&format!("\nModerator: {m}"));
        }
    }
    truncate(&out, 2000)
}

pub async fn get(db: &PgPool, guild_id: i64, kind: CaseKind) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar("SELECT template FROM dm_templates WHERE guild_id = $1 AND kind = $2")
        .bind(guild_id)
        .bind(kind.as_str())
        .fetch_optional(db)
        .await
}

pub async fn set(db: &PgPool, guild_id: i64, kind: CaseKind, text: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO dm_templates (guild_id, kind, template) VALUES ($1, $2, $3) \
         ON CONFLICT (guild_id, kind) DO UPDATE SET template = EXCLUDED.template",
    )
    .bind(guild_id)
    .bind(kind.as_str())
    .bind(text)
    .execute(db)
    .await?;
    Ok(())
}

pub async fn reset(db: &PgPool, guild_id: i64, kind: CaseKind) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM dm_templates WHERE guild_id = $1 AND kind = $2")
        .bind(guild_id)
        .bind(kind.as_str())
        .execute(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_rules() {
        let v = TemplateVars { action: "Warning", reason: "spam", duration: None, server: "Den", case_number: 4, moderator: None };
        assert_eq!(render(default_template(CaseKind::Warn), &v), "You were warned in **Den**.\nReason: spam\nCase #4");
        assert_eq!(render("{moderator}", &v), "Staff");
        assert_eq!(render("Hi", &TemplateVars { moderator: Some("Tan"), ..v }), "Hi\nModerator: Tan");
        assert_eq!(render("{reason}", &TemplateVars { reason: &"x".repeat(3000), ..v }).chars().count(), 2000);
    }
}
