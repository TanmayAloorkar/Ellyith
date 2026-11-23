//! `/healthcheck`: a read-only report on how well the server is set up for safety.
use crate::invoker::Invoker;
use crate::logging::{self, LogCategory};
use crate::moderation::perms::require_manage_server;
use crate::reply::Tone;
use crate::safety::logic::{health_lines, Health, HealthInput, HealthLine};
use crate::safety::quarantine::{bot_top, require_setup};
use crate::{settings, safety::store, Error};
use poise::serenity_prelude::{CreateEmbed, MfaLevel, Permissions, RoleId, VerificationLevel};

fn lines(v: &[HealthLine]) -> String {
    v.iter()
        .map(|(h, label, text)| {
            let mark = match h {
                Health::Bad => "✖",
                Health::Warn => "⚠",
                Health::Ok => "✔",
            };
            format!("{mark} **{label}** {text}")
        })
        .collect::<Vec<_>>()
        .join("
")
}

pub fn render(problems: Vec<HealthLine>, setup: Vec<HealthLine>) -> CreateEmbed {
    let title = match problems.len() {
        0 => "🩺 Server health · all good".to_string(),
        1 => "🩺 Server health · 1 problem".to_string(),
        n => format!("🩺 Server health · {n} problems"),
    };
    let none = || "None found.".to_string();
    let cut = |s: String| s.chars().take(1024).collect::<String>();
    let (p, s) = (lines(&problems), lines(&setup));
    CreateEmbed::new()
        .colour(Tone::Safety.colour())
        .title(title)
        .field("Problems", cut(if p.is_empty() { none() } else { p }), false)
        .field("Setup", cut(if s.is_empty() { none() } else { s }), false)
}

pub async fn health_core(inv: &Invoker<'_>) -> Result<CreateEmbed, Error> {
    require_manage_server(inv).await?;
    let gid = inv.guild_id.get() as i64;
    let top = bot_top(inv.ctx, inv.guild_id).await?;
    let mod_ids = settings::get_or_create(&inv.data.db, gid).await?.mod_roles;
    let logs = logging::list(&inv.data.db, gid).await?;
    let s = store::get_settings(&inv.data.db, gid).await?;
    let nuke_on = crate::protection::store::get_settings(&inv.data.db, gid).await?.nuke_enabled;
    let bot_id = inv.ctx.cache.current_user().id;

    // Copy everything out of the cache guard before any await.
    let mut h = {
        let g = inv.ctx.cache.guild(inv.guild_id).ok_or("guild not in cache")?;
        let everyone = g.roles.get(&RoleId::new(inv.guild_id.get())).map(|r| r.permissions).unwrap_or_default();
        let is_mod = |id: &RoleId| mod_ids.contains(&(id.get() as i64));
        // Managed roles count too (an integration can hold Administrator), except the bot's own.
        let roles = g.roles.values().filter(|r| r.id.get() != inv.guild_id.get() && r.tags.bot_id != Some(bot_id)).map(|r| (r.name.to_string(), r.permissions, is_mod(&r.id))).collect();
        let mod_roles = g.roles.values().filter(|r| is_mod(&r.id)).map(|r| r.name.to_string()).collect();
        let admin: Vec<RoleId> = g.roles.values().filter(|r| r.permissions.administrator()).map(|r| r.id).collect();
        // Admin members: cached members holding an Administrator role; the owner counts only if they hold one.
        let admin_members = g.members.values().filter(|m| m.roles.iter().any(|r| admin.contains(r))).count();
        let unmoderatable = g
            .members
            .values()
            .filter(|m| m.user.id != g.owner_id && m.user.id != bot_id)
            .filter(|m| m.roles.iter().filter_map(|r| g.roles.get(r)).map(|r| r.position).max().unwrap_or(0) >= top)
            .count();
        let bot_perms = g.members.get(&bot_id).map(|m| g.member_permissions(m)).unwrap_or_else(Permissions::empty);
        let mut bot_missing = vec![];
        if !bot_perms.contains(Permissions::MANAGE_ROLES) {
            bot_missing.push("Manage Roles");
        }
        if !bot_perms.contains(Permissions::MANAGE_CHANNELS) {
            bot_missing.push("Manage Channels");
        }
        HealthInput {
            everyone_perms: everyone,
            roles,
            admin_roles: admin.len(),
            admin_members,
            unmoderatable,
            mfa_required: g.mfa_level == MfaLevel::Elevated,
            verification_level_medium_or_higher: matches!(g.verification_level, VerificationLevel::Medium | VerificationLevel::High | VerificationLevel::Higher),
            mod_roles,
            unset_logs: vec![],
            verify_on_without_quarantine: s.verify_mode != "off" && require_setup(&s, &g).is_err(),
            lockdown_channels_empty: s.lockdown_channels.is_empty(),
            bot_missing,
            bot_not_top: nuke_on && g.roles.values().any(|r| r.position > top),
        }
    };
    h.unset_logs = LogCategory::ALL.into_iter().filter(|c| !logs.iter().any(|(l, _)| l == c)).map(LogCategory::as_str).collect();
    let (problems, setup) = health_lines(&h);
    Ok(render(problems, setup))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::safety::logic::Health::{Bad, Ok, Warn};

    fn f(e: CreateEmbed) -> serde_json::Value {
        serde_json::to_value(e).unwrap()
    }

    #[test]
    fn render_all_good() {
        let e = f(render(vec![], vec![(Ok, "Mod roles".into(), "Mod".into())]));
        assert_eq!(e["title"], "🩺 Server health · all good");
        assert_eq!(e["color"], 0xbaa3ff);
        assert_eq!(e["fields"][1]["name"], "Setup");
        assert_eq!(e["fields"][1]["value"], "✔ **Mod roles** Mod");
    }

    #[test]
    fn render_problems() {
        let p = vec![(Bad, "A".into(), "a".into()), (Warn, "B".into(), "b".into()), (Bad, "C".into(), "c".into())];
        let e = f(render(p, vec![(Warn, "S".into(), "s".into())]));
        assert_eq!(e["title"], "🩺 Server health · 3 problems");
        assert_eq!(e["fields"][0]["name"], "Problems");
        assert_eq!(e["fields"][0]["value"], "✖ **A** a\n⚠ **B** b\n✖ **C** c");
        assert_eq!(e["fields"][1]["value"], "⚠ **S** s");
    }

    #[test]
    fn render_one_problem_singular() {
        let e = f(render(vec![(Bad, "A".into(), "a".into())], vec![]));
        assert_eq!(e["title"], "🩺 Server health · 1 problem");
    }
}
