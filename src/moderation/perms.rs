use crate::settings::{self, GuildSettings};
use crate::invoker::Invoker;
use crate::{Error, UserError};
use poise::serenity_prelude::{self as serenity, GuildId, Member, Permissions, UserId};

#[derive(Debug, Clone, PartialEq)]
pub struct MemberFacts {
    pub user_id: u64,
    pub is_owner: bool,
    pub permissions: serenity::Permissions,
    pub role_ids: Vec<u64>,
    pub top_role_position: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denied {
    NotModerator,
    TargetIsSelf,
    TargetIsOwner,
    TargetProtected,
    TargetNotBelowYou,
    TargetNotBelowBot,
}

impl Denied {
    pub fn message(&self) -> &'static str {
        match self {
            Denied::NotModerator => "You don't have permission to use this command.",
            Denied::TargetIsSelf => "You can't do that to yourself.",
            Denied::TargetIsOwner => "You can't do that to the server owner.",
            Denied::TargetProtected => "That user has a protected role.",
            Denied::TargetNotBelowYou => "That user's highest role is not below yours.",
            Denied::TargetNotBelowBot => "That user's highest role is not below mine.",
        }
    }
}

fn has_role(m: &MemberFacts, roles: &[i64]) -> bool {
    m.role_ids.iter().any(|r| roles.contains(&(*r as i64)))
}

fn is_admin(m: &MemberFacts) -> bool {
    m.is_owner || m.permissions.contains(Permissions::ADMINISTRATOR)
}

pub fn can_moderate(invoker: &MemberFacts, s: &GuildSettings, required: Permissions) -> bool {
    is_admin(invoker) || invoker.permissions.contains(required) || has_role(invoker, &s.mod_roles)
}

pub fn can_target(
    invoker: &MemberFacts,
    target: Option<&MemberFacts>,
    bot: &MemberFacts,
    s: &GuildSettings,
) -> Result<(), Denied> {
    let Some(t) = target else { return Ok(()) };
    if t.user_id == invoker.user_id {
        return Err(Denied::TargetIsSelf);
    }
    if t.is_owner {
        return Err(Denied::TargetIsOwner);
    }
    if !is_admin(invoker) && has_role(t, &s.protected_roles) {
        return Err(Denied::TargetProtected);
    }
    if !invoker.is_owner && t.top_role_position >= invoker.top_role_position {
        return Err(Denied::TargetNotBelowYou);
    }
    if t.top_role_position >= bot.top_role_position {
        return Err(Denied::TargetNotBelowBot);
    }
    Ok(())
}

pub fn is_exempt_from_auto(m: &MemberFacts, s: &GuildSettings) -> bool {
    is_admin(m) || has_role(m, &s.protected_roles)
}

pub async fn member_facts(
    ctx: &serenity::Context,
    guild_id: GuildId,
    member: &Member,
) -> Result<MemberFacts, Error> {
    let g = ctx.cache.guild(guild_id).ok_or("guild not in cache")?;
    let top = member
        .roles
        .iter()
        .filter_map(|r| g.roles.get(r))
        .map(|r| r.position)
        .max()
        .unwrap_or(0);
    Ok(MemberFacts {
        user_id: member.user.id.get(),
        is_owner: g.owner_id == member.user.id,
        permissions: g.member_permissions(member),
        role_ids: member.roles.iter().map(|r| r.get()).collect(),
        top_role_position: top,
    })
}

pub async fn require_member(inv: &Invoker<'_>, user: UserId) -> Result<Member, Error> {
    match inv.guild_id.member(inv.ctx, user).await {
        Ok(m) => Ok(m),
        Err(e) if is_absent(&e) => Err(UserError("That user is not in this server.".into()).into()),
        Err(e) => Err(e.into()),
    }
}

/// Discord says the user is not a member: Unknown Member (10007) or Unknown User (10013).
/// Any other failure (rate limit, 5xx) must not be read as "absent".
pub(crate) fn is_absent(e: &serenity::Error) -> bool {
    matches!(e, serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(r)) if is_unknown_member_code(r.error.code))
}

fn is_unknown_member_code(code: isize) -> bool {
    matches!(code, 10007 | 10013)
}

/// Discord says the ban does not exist: HTTP 404 or Unknown Ban (10026).
pub(crate) fn is_unknown_ban(e: &serenity::Error) -> bool {
    matches!(e, serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(r)) if r.status_code.as_u16() == 404 || is_unknown_ban_code(r.error.code))
}

fn is_unknown_ban_code(code: isize) -> bool {
    code == 10026
}

pub async fn authorize(
    inv: &Invoker<'_>,
    required: Permissions,
    target: Option<UserId>,
) -> Result<GuildSettings, Error> {
    let guild_id = inv.guild_id;
    let s = settings::get_or_create(&inv.data.db, guild_id.get() as i64).await?;
    let sctx = inv.ctx;
    let invoker_m = require_member(inv, inv.user.id).await?;
    let invoker = member_facts(sctx, guild_id, &invoker_m).await?;
    if !can_moderate(&invoker, &s, required) {
        return Err(UserError(Denied::NotModerator.message().into()).into());
    }
    let bot_id = sctx.cache.current_user().id;
    let bot_m = require_member(inv, bot_id).await?;
    let bot = member_facts(sctx, guild_id, &bot_m).await?;
    // Target not in server: nothing to rank-check (e.g. banning an absent user).
    let target_facts = match target {
        Some(id) => match guild_id.member(sctx, id).await {
            Ok(m) => Some(member_facts(sctx, guild_id, &m).await?),
            Err(e) if is_absent(&e) => None,
            Err(e) => return Err(e.into()),
        },
        None => None,
    };
    can_target(&invoker, target_facts.as_ref(), &bot, &s).map_err(|d| UserError(d.message().into()))?;
    Ok(s)
}

/// Owner, Administrator or Manage Server.
pub async fn require_manage_server(inv: &Invoker<'_>) -> Result<(), Error> {
    let denied = || UserError("You need Manage Server to use /config.".into());
    let m = require_member(inv, inv.user.id).await.map_err(|_| denied())?;
    let ok = inv.ctx.cache.guild(inv.guild_id).is_some_and(|g| {
        g.owner_id == m.user.id || g.member_permissions(&m).manage_guild()
    });
    if ok { Ok(()) } else { Err(denied().into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(id: u64, top: u16) -> MemberFacts {
        MemberFacts { user_id: id, is_owner: false, permissions: Permissions::empty(), role_ids: vec![], top_role_position: top }
    }

    #[test]
    fn moderate() {
        let s = GuildSettings::defaults(1);
        assert!(!can_moderate(&facts(1, 5), &s, Permissions::BAN_MEMBERS));
        assert!(can_moderate(&MemberFacts { permissions: Permissions::BAN_MEMBERS, ..facts(1, 5) }, &s, Permissions::BAN_MEMBERS));
        assert!(can_moderate(&MemberFacts { role_ids: vec![42], ..facts(1, 5) }, &GuildSettings { mod_roles: vec![42], ..s.clone() }, Permissions::BAN_MEMBERS));
    }

    #[test]
    fn target() {
        let s = GuildSettings::defaults(1);
        let bot = facts(99, 10);
        assert_eq!(can_target(&facts(1, 5), None, &bot, &s), Ok(()));
        assert_eq!(can_target(&facts(1, 5), Some(&facts(1, 5)), &bot, &s), Err(Denied::TargetIsSelf));
        assert_eq!(can_target(&facts(1, 5), Some(&facts(2, 5)), &bot, &s), Err(Denied::TargetNotBelowYou));
        assert_eq!(can_target(&facts(1, 20), Some(&facts(2, 12)), &bot, &s), Err(Denied::TargetNotBelowBot));
        let prot = GuildSettings { protected_roles: vec![7], ..s.clone() };
        assert_eq!(can_target(&facts(1, 5), Some(&MemberFacts { role_ids: vec![7], ..facts(2, 1) }), &bot, &prot), Err(Denied::TargetProtected));
        let admin = MemberFacts { permissions: Permissions::ADMINISTRATOR, ..facts(1, 5) };
        assert_eq!(can_target(&admin, Some(&MemberFacts { role_ids: vec![7], ..facts(2, 1) }), &bot, &prot), Ok(()));
        assert_eq!(can_target(&facts(1, 5), Some(&MemberFacts { is_owner: true, ..facts(2, 1) }), &bot, &s), Err(Denied::TargetIsOwner));
    }

    // ErrorResponse/DiscordJsonError are non_exhaustive, so only the code check is testable.
    #[test]
    fn absent_only_on_unknown_member_codes() {
        assert!(is_unknown_member_code(10007));
        assert!(is_unknown_member_code(10013));
        assert!(!is_unknown_member_code(0));
        assert!(!is_unknown_member_code(20028)); // rate limit
        assert!(!is_absent(&serenity::Error::Other("x")));
    }

    #[test]
    fn unknown_ban_code_only() {
        assert!(is_unknown_ban_code(10026));
        assert!(!is_unknown_ban_code(10007));
        assert!(!is_unknown_ban(&serenity::Error::Other("x")));
    }

    #[test]
    fn exempt() {
        let prot = GuildSettings { protected_roles: vec![7], ..GuildSettings::defaults(1) };
        assert!(is_exempt_from_auto(&MemberFacts { role_ids: vec![7], ..facts(2, 1) }, &prot));
        assert!(!is_exempt_from_auto(&facts(2, 1), &prot));
    }
}
