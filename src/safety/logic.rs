use crate::safety::store::SavedOverwrite;
use crate::UserError;
use poise::serenity_prelude::Permissions;

pub struct RoleInfo {
    pub id: u64,
    pub position: u16,
    pub managed: bool,
}

pub fn roles_to_save(member_roles: &[RoleInfo], bot_top: u16) -> (Vec<u64>, Vec<u64>) {
    let (kept, saved): (Vec<_>, Vec<_>) = member_roles.iter().partition(|r| r.managed || r.position >= bot_top);
    (saved.iter().map(|r| r.id).collect(), kept.iter().map(|r| r.id).collect())
}

pub fn quiz_passes(expected: &[String], answers: &[String]) -> bool {
    expected.iter().enumerate().all(|(i, e)| {
        let e = e.trim().to_lowercase();
        e.is_empty() || answers.get(i).is_some_and(|a| a.trim().to_lowercase().contains(&e))
    })
}

pub fn parse_question_line(s: &str) -> Result<Option<(String, String)>, UserError> {
    if s.trim().is_empty() {
        return Ok(None);
    }
    let (q, a) = s.split_once('|').unwrap_or((s, ""));
    let (q, a) = (q.trim(), a.trim());
    if q.is_empty() {
        return Err(UserError("Each line needs a question.".into()));
    }
    if q.chars().count() > 45 {
        return Err(UserError("Questions can be at most 45 characters.".into()));
    }
    Ok(Some((q.to_string(), a.to_string())))
}

pub const LOCK_DENY: Permissions = Permissions::SEND_MESSAGES
    .union(Permissions::SEND_MESSAGES_IN_THREADS)
    .union(Permissions::CREATE_PUBLIC_THREADS)
    .union(Permissions::CREATE_PRIVATE_THREADS)
    .union(Permissions::ADD_REACTIONS);
pub const LOCK_DENY_VOICE: Permissions = Permissions::CONNECT.union(Permissions::SPEAK);
pub const STAFF_ALLOW: Permissions = Permissions::SEND_MESSAGES.union(Permissions::SEND_MESSAGES_IN_THREADS);
pub const STAFF_ALLOW_VOICE: Permissions = Permissions::CONNECT.union(Permissions::SPEAK);
pub const BOT_ALLOW: Permissions =
    Permissions::VIEW_CHANNEL.union(Permissions::SEND_MESSAGES).union(Permissions::EMBED_LINKS);

pub fn lockdown_overwrites(
    existing: &[SavedOverwrite],
    everyone_role: u64,
    mod_roles: &[u64],
    bot_user: u64,
    is_voice: bool,
) -> Vec<SavedOverwrite> {
    let mut deny = LOCK_DENY;
    let mut staff = STAFF_ALLOW;
    if is_voice {
        deny |= LOCK_DENY_VOICE;
        staff |= STAFF_ALLOW_VOICE;
    }
    // (allow bits to add, deny bits to add) for a target, or None to leave it alone
    let change = |id: u64, is_member: bool, allow: Permissions| -> Option<(Permissions, Permissions)> {
        if is_member {
            (id == bot_user).then_some((BOT_ALLOW, Permissions::empty()))
        } else if mod_roles.contains(&id) {
            Some((staff, Permissions::empty()))
        } else if id == everyone_role || allow.contains(Permissions::SEND_MESSAGES) {
            Some((Permissions::empty(), deny))
        } else {
            None
        }
    };
    let apply = |o: &SavedOverwrite, add_allow: Permissions, add_deny: Permissions| SavedOverwrite {
        allow: ((Permissions::from_bits_truncate(o.allow as u64) | add_allow) - add_deny).bits() as i64,
        deny: ((Permissions::from_bits_truncate(o.deny as u64) | add_deny) - add_allow).bits() as i64,
        ..o.clone()
    };
    let mut out: Vec<SavedOverwrite> = existing
        .iter()
        .map(|o| match change(o.target_id as u64, o.is_member, Permissions::from_bits_truncate(o.allow as u64)) {
            Some((a, d)) => apply(o, a, d),
            None => o.clone(),
        })
        .collect();
    let wanted = std::iter::once((everyone_role, false))
        .chain(mod_roles.iter().map(|&r| (r, false)))
        .chain(std::iter::once((bot_user, true)));
    for (id, is_member) in wanted {
        if !out.iter().any(|o| o.target_id == id as i64 && o.is_member == is_member) {
            let blank = SavedOverwrite { target_id: id as i64, is_member, allow: 0, deny: 0 };
            let (a, d) = change(id, is_member, Permissions::empty()).expect("wanted target always changes");
            out.push(apply(&blank, a, d));
        }
    }
    out
}

pub fn restore_plan(saved: &[SavedOverwrite], current: &[SavedOverwrite]) -> (Vec<SavedOverwrite>, Vec<SavedOverwrite>) {
    let del = current
        .iter()
        .filter(|c| !saved.iter().any(|s| s.target_id == c.target_id && s.is_member == c.is_member))
        .cloned()
        .collect();
    (saved.to_vec(), del)
}

pub struct HealthInput {
    pub everyone_perms: Permissions,
    pub roles: Vec<(String, Permissions, bool)>,
    pub admin_roles: usize,
    pub admin_members: usize,
    pub unmoderatable: usize,
    pub mfa_required: bool,
    pub verification_level_medium_or_higher: bool,
    pub mod_roles: Vec<String>,
    pub unset_logs: Vec<&'static str>,
    pub verify_on_without_quarantine: bool,
    pub lockdown_channels_empty: bool,
    pub bot_missing: Vec<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Health {
    Bad,
    Warn,
    Ok,
}

pub type HealthLine = (Health, String, String);

const DANGEROUS: Permissions = Permissions::ADMINISTRATOR
    .union(Permissions::MANAGE_GUILD)
    .union(Permissions::MANAGE_ROLES)
    .union(Permissions::MANAGE_CHANNELS)
    .union(Permissions::KICK_MEMBERS)
    .union(Permissions::BAN_MEMBERS)
    .union(Permissions::MANAGE_WEBHOOKS)
    .union(Permissions::MENTION_EVERYONE);

pub fn health_lines(h: &HealthInput) -> (Vec<HealthLine>, Vec<HealthLine>) {
    use Health::{Bad, Ok, Warn};
    let mut probs: Vec<HealthLine> = vec![];
    if h.everyone_perms.intersects(DANGEROUS) {
        probs.push((Bad, "@everyone has dangerous permissions".into(), "Everyone in the server can use them. Remove them from @everyone.".into()));
    }
    for (name, perms, is_mod) in &h.roles {
        if !is_mod && perms.intersects(DANGEROUS) {
            probs.push((Bad, format!("{name} has dangerous permissions"), "It is not a mod role. Check who holds it.".into()));
        }
    }
    if h.unmoderatable > 0 {
        probs.push((Warn, "Members the bot cannot moderate".into(), format!("{} members sit above the bot's top role.", h.unmoderatable)));
    }
    if !h.bot_missing.is_empty() {
        probs.push((Bad, "Bot is missing permissions".into(), h.bot_missing.join(", ")));
    }

    let line = |bad: bool, title: &str, good: &str, fix: &str| {
        if bad { (Warn, title.to_string(), fix.to_string()) } else { (Ok, title.to_string(), good.to_string()) }
    };
    let setup = vec![
        line(h.mod_roles.is_empty(), "Mod roles", &h.mod_roles.join(", "), "None set. Use /config mod-role."),
        line(!h.unset_logs.is_empty(), "Log channels", "All set.", &format!("Not set: {}", h.unset_logs.join(", "))),
        line(!h.mfa_required, "2FA for moderators", "Required.", "Not required for moderator actions."),
        line(!h.verification_level_medium_or_higher, "Verification level", "Medium or higher.", "Below medium. Raise it in server settings."),
        line(h.admin_members > 5, "Administrators", &format!("{} roles, {} members.", h.admin_roles, h.admin_members), &format!("{} members have Administrator.", h.admin_members)),
        line(h.verify_on_without_quarantine, "Quarantine for verification", "Ready.", "Verification is on but no quarantine role is set."),
        line(h.lockdown_channels_empty, "Lockdown channels", "Set.", "No channels listed for /lockdown server."),
    ];
    (probs, setup)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ow(id: i64, member: bool, allow: Permissions, deny: Permissions) -> SavedOverwrite {
        SavedOverwrite { target_id: id, is_member: member, allow: allow.bits() as i64, deny: deny.bits() as i64 }
    }
    fn p(x: i64) -> Permissions {
        Permissions::from_bits_truncate(x as u64)
    }
    fn find(v: &[SavedOverwrite], id: i64, member: bool) -> &SavedOverwrite {
        v.iter().find(|o| o.target_id == id && o.is_member == member).unwrap()
    }

    #[test]
    fn roles_to_save_keeps_managed_and_high() {
        let r = |id, position, managed| RoleInfo { id, position, managed };
        let (saved, kept) = roles_to_save(&[r(1, 5, false), r(2, 9, false), r(3, 3, true), r(4, 10, false)], 9);
        assert_eq!(saved, vec![1]);
        assert_eq!(kept, vec![2, 3, 4]);
    }

    #[test]
    fn quiz_passes_rules() {
        let s = |x: &str| x.to_string();
        assert!(quiz_passes(&[s("rule 3"), s("")], &[s("  RULE 3 is no spam"), s("anything")]));
        assert!(!quiz_passes(&[s("rule 3")], &[s("no")]));
    }

    #[test]
    fn parse_question_line_limits() {
        assert_eq!(parse_question_line("How old? | 13").unwrap(), Some(("How old?".into(), "13".into())));
        assert_eq!(parse_question_line("Why join?").unwrap(), Some(("Why join?".into(), "".into())));
        assert_eq!(parse_question_line("   ").unwrap(), None);
        assert!(parse_question_line(" | x").is_err());
        assert!(parse_question_line(&"a".repeat(46)).is_err());
        assert!(parse_question_line(&"a".repeat(45)).is_ok());
    }

    fn base() -> Vec<SavedOverwrite> {
        vec![
            ow(1, false, Permissions::empty(), Permissions::empty()),
            ow(2, false, Permissions::SEND_MESSAGES, Permissions::empty()),
            ow(3, false, Permissions::VIEW_CHANNEL, Permissions::empty()),
        ]
    }

    #[test]
    fn lockdown_denies_explicit_allows() {
        let out = lockdown_overwrites(&base(), 1, &[4], 99, false);
        assert!(p(find(&out, 1, false).deny).contains(LOCK_DENY));
        let r2 = find(&out, 2, false);
        assert!(p(r2.deny).contains(LOCK_DENY));
        assert!(!p(r2.allow).contains(Permissions::SEND_MESSAGES));
        assert_eq!(find(&out, 3, false), &base()[2]);
        assert!(p(find(&out, 4, false).allow).contains(STAFF_ALLOW));
        assert!(p(find(&out, 99, true).allow).contains(BOT_ALLOW));
    }

    #[test]
    fn lockdown_voice_adds_connect_speak() {
        let out = lockdown_overwrites(&base(), 1, &[4], 99, true);
        let cs = Permissions::CONNECT | Permissions::SPEAK;
        assert!(p(find(&out, 1, false).deny).contains(cs));
        assert!(p(find(&out, 4, false).allow).contains(cs));
    }

    #[test]
    fn lockdown_mod_role_deny_cleared() {
        let mut e = base();
        e.push(ow(4, false, Permissions::empty(), Permissions::SEND_MESSAGES));
        let out = lockdown_overwrites(&e, 1, &[4], 99, false);
        let m = find(&out, 4, false);
        assert!(p(m.allow).contains(Permissions::SEND_MESSAGES));
        assert!(!p(m.deny).contains(Permissions::SEND_MESSAGES));
    }

    #[test]
    fn lockdown_restore_plan_exact() {
        let a = ow(1, false, Permissions::VIEW_CHANNEL, Permissions::empty());
        let b = ow(2, true, Permissions::empty(), Permissions::ADD_REACTIONS);
        let a2 = ow(1, false, Permissions::VIEW_CHANNEL, LOCK_DENY);
        let b2 = ow(2, true, Permissions::SEND_MESSAGES, Permissions::empty());
        let c = ow(5, false, STAFF_ALLOW, Permissions::empty());
        let (set, del) = restore_plan(&[a.clone(), b.clone()], &[a2, b2, c.clone()]);
        assert_eq!(set, vec![a, b]);
        assert_eq!(del, vec![c]);
    }

    fn good() -> HealthInput {
        HealthInput {
            everyone_perms: Permissions::VIEW_CHANNEL,
            roles: vec![("Mod".into(), Permissions::ADMINISTRATOR, true), ("Member".into(), Permissions::empty(), false)],
            admin_roles: 1,
            admin_members: 2,
            unmoderatable: 0,
            mfa_required: true,
            verification_level_medium_or_higher: true,
            mod_roles: vec!["Mod".into()],
            unset_logs: vec![],
            verify_on_without_quarantine: false,
            lockdown_channels_empty: false,
            bot_missing: vec![],
        }
    }

    #[test]
    fn health_lines_flags() {
        let (probs, setup) = health_lines(&good());
        assert!(probs.is_empty(), "{probs:?}");
        assert!(setup.iter().all(|l| l.0 == Health::Ok), "{setup:?}");

        let mut h = good();
        h.everyone_perms = Permissions::MENTION_EVERYONE;
        let (probs, _) = health_lines(&h);
        assert_eq!(probs.len(), 1);
        assert_eq!(probs[0].0, Health::Bad);
        assert!(probs[0].1.contains("@everyone"));

        let mut h = good();
        h.roles.push(("Helper".into(), Permissions::ADMINISTRATOR, false));
        let (probs, _) = health_lines(&h);
        assert_eq!(probs.len(), 1);
        assert_eq!(probs[0].0, Health::Bad);
        assert!(probs[0].1.contains("Helper"));

        let mut h = good();
        h.unmoderatable = 12;
        let (probs, _) = health_lines(&h);
        assert!(probs.iter().any(|l| l.0 == Health::Warn && (l.1.contains("12") || l.2.contains("12"))));

        let mut h = good();
        h.mfa_required = false;
        let (_, setup) = health_lines(&h);
        assert!(setup.iter().any(|l| l.0 == Health::Warn));
    }
}
