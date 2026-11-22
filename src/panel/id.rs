use crate::logging::LogCategory;
use crate::moderation::cases::CaseKind;
use crate::moderation::commands::config_core::RoleList;
use poise::serenity_prelude::{ChannelId, UserId};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Members,
    NamesVoice,
    Channels,
    Safety,
    Records,
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Warn,
    Timeout,
    Untimeout,
    Kick,
    Tempban,
    Ban,
    Unban,
    NickSet,
    NickReset,
    Disconnect,
    VoiceMute,
    VoiceUnmute,
    Move,
    Purge,
    History,
    CaseView,
    CaseReason,
    CaseRemove,
    NoteAdd,
    NoteList,
    NoteView,
    NoteDelete,
    Quarantine,
    Release,
    WaitingReview,
    HealthCheck,
    Lockdown,
    EndLockdown,
}

/// Config sub-views opened from the Config category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    ModRoles,
    Protected,
    Logs,
    Dm,
    Ladder,
    Decay,
    Filter,
}

/// Config pop-ups: the button opens it, the submit carries the same token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    LadderAdd,
    LadderRemove,
    DecayOn,
    Words,
    Hoist,
    Fallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigId {
    /// Sub-view button.
    Open(Page),
    /// Role select, saved on select.
    Roles(RoleList),
    /// Log category string select.
    LogCat,
    /// Log channel select for a category.
    /// `None` = all categories.
    LogChan(Option<LogCategory>),
    LogOff(Option<LogCategory>),
    /// DM kind string select.
    DmKind,
    /// DM text pop-up submit.
    DmText(CaseKind),
    /// Show-moderator toggle carrying the new value.
    DmShowMod(bool),
    /// Button that opens a pop-up.
    Form(Form),
    /// Pop-up submit.
    Submit(Form),
    DecayOff,
    /// Filter on/off button carrying the new value.
    FilterSet(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelId {
    /// Back to the category list.
    Home,
    Category(Category),
    Action(Action),
    /// User select.
    PickUser(Action),
    /// Channel select (Move carries the user, Purge none).
    PickChannel(Action, Option<UserId>),
    /// Pop-up submit.
    Modal(Action, Option<UserId>, Option<ChannelId>),
    Config(ConfigId),
    /// Quarantine review card: approve (true) or deny, for this user.
    Review(bool, UserId),
    /// Public Verify button on the verify message.
    Verify,
    /// Quiz pop-up submit.
    VerifyQuiz,
    /// Waiting-review member select.
    ReviewPick,
}

const PAGES: [(Page, &str); 7] = [
    (Page::ModRoles, "modroles"),
    (Page::Protected, "protected"),
    (Page::Logs, "logs"),
    (Page::Dm, "dm"),
    (Page::Ladder, "ladder"),
    (Page::Decay, "decay"),
    (Page::Filter, "filter"),
];

const FORMS: [(Form, &str); 6] = [
    (Form::LadderAdd, "ladadd"),
    (Form::LadderRemove, "ladrem"),
    (Form::DecayOn, "decayon"),
    (Form::Words, "words"),
    (Form::Hoist, "hoist"),
    (Form::Fallback, "fallback"),
];

const ROLE_LISTS: [(RoleList, &str); 2] = [(RoleList::Mod, "mod"), (RoleList::Protected, "prot")];

fn tok<T: Copy + PartialEq>(table: &[(T, &'static str)], v: T) -> &'static str {
    table.iter().find(|(x, _)| *x == v).unwrap().1
}

fn untok<T: Copy>(table: &[(T, &'static str)], s: &str) -> Option<T> {
    table.iter().find(|(_, t)| *t == s).map(|(x, _)| *x)
}

/// Token for a log category; `all` is `None`.
pub fn log_tok(c: Option<LogCategory>) -> &'static str {
    c.map_or("all", |c| c.as_str())
}

/// Outer `None` = unknown token.
pub fn log_cat(s: &str) -> Option<Option<LogCategory>> {
    if s == "all" {
        return Some(None);
    }
    LogCategory::ALL.into_iter().find(|c| c.as_str() == s).map(Some)
}

fn flag(s: &str) -> Option<bool> {
    match s {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

impl ConfigId {
    fn encode(&self) -> String {
        match self {
            ConfigId::Open(p) => format!("mp:g:o:{}", tok(&PAGES, *p)),
            ConfigId::Roles(r) => format!("mp:g:r:{}", tok(&ROLE_LISTS, *r)),
            ConfigId::LogCat => "mp:g:lc".into(),
            ConfigId::LogChan(c) => format!("mp:g:lh:{}", log_tok(*c)),
            ConfigId::LogOff(c) => format!("mp:g:lo:{}", log_tok(*c)),
            ConfigId::DmKind => "mp:g:dk".into(),
            ConfigId::DmText(k) => format!("mp:g:dt:{}", k.as_str()),
            ConfigId::DmShowMod(v) => format!("mp:g:ds:{}", *v as u8),
            ConfigId::Form(f) => format!("mp:g:f:{}", tok(&FORMS, *f)),
            ConfigId::Submit(f) => format!("mp:g:s:{}", tok(&FORMS, *f)),
            ConfigId::DecayOff => "mp:g:do".into(),
            ConfigId::FilterSet(v) => format!("mp:g:fi:{}", *v as u8),
        }
    }

    fn decode(parts: &[&str]) -> Option<Self> {
        match parts {
            ["o", p] => untok(&PAGES, p).map(ConfigId::Open),
            ["r", r] => untok(&ROLE_LISTS, r).map(ConfigId::Roles),
            ["lc"] => Some(ConfigId::LogCat),
            ["lh", c] => log_cat(c).map(ConfigId::LogChan),
            ["lo", c] => log_cat(c).map(ConfigId::LogOff),
            ["dk"] => Some(ConfigId::DmKind),
            ["dt", k] => CaseKind::from_str(k).ok().map(ConfigId::DmText),
            ["ds", v] => flag(v).map(ConfigId::DmShowMod),
            ["f", f] => untok(&FORMS, f).map(ConfigId::Form),
            ["s", f] => untok(&FORMS, f).map(ConfigId::Submit),
            ["do"] => Some(ConfigId::DecayOff),
            ["fi", v] => flag(v).map(ConfigId::FilterSet),
            _ => None,
        }
    }
}

const CATEGORIES: [(Category, &str); 6] = [
    (Category::Members, "members"),
    (Category::NamesVoice, "names"),
    (Category::Channels, "channels"),
    (Category::Safety, "safety"),
    (Category::Records, "records"),
    (Category::Config, "config"),
];

const ACTIONS: [(Action, &str); 28] = [
    (Action::Warn, "warn"),
    (Action::Timeout, "timeout"),
    (Action::Untimeout, "untimeout"),
    (Action::Kick, "kick"),
    (Action::Tempban, "tempban"),
    (Action::Ban, "ban"),
    (Action::Unban, "unban"),
    (Action::NickSet, "nickset"),
    (Action::NickReset, "nickreset"),
    (Action::Disconnect, "disconnect"),
    (Action::VoiceMute, "vmute"),
    (Action::VoiceUnmute, "vunmute"),
    (Action::Move, "move"),
    (Action::Purge, "purge"),
    (Action::History, "history"),
    (Action::CaseView, "caseview"),
    (Action::CaseReason, "casereason"),
    (Action::CaseRemove, "caseremove"),
    (Action::NoteAdd, "noteadd"),
    (Action::NoteList, "notelist"),
    (Action::NoteView, "noteview"),
    (Action::NoteDelete, "notedelete"),
    (Action::Quarantine, "quarantine"),
    (Action::Release, "release"),
    (Action::WaitingReview, "review"),
    (Action::HealthCheck, "health"),
    (Action::Lockdown, "lockdown"),
    (Action::EndLockdown, "unlock"),
];

impl Category {
    fn token(self) -> &'static str {
        CATEGORIES.iter().find(|(c, _)| *c == self).unwrap().1
    }
    fn parse(s: &str) -> Option<Self> {
        CATEGORIES.iter().find(|(_, t)| *t == s).map(|(c, _)| *c)
    }
}

impl Action {
    fn token(self) -> &'static str {
        ACTIONS.iter().find(|(a, _)| *a == self).unwrap().1
    }
    fn parse(s: &str) -> Option<Self> {
        ACTIONS.iter().find(|(_, t)| *t == s).map(|(a, _)| *a)
    }
}

fn opt(id: Option<u64>) -> String {
    id.map_or("-".into(), |v| v.to_string())
}

fn parse_opt(s: &str) -> Option<Option<u64>> {
    if s == "-" {
        return Some(None);
    }
    // Reject "0", signs and leading zeros so every id has one encoding.
    if s.starts_with('0') || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok().map(Some)
}

impl PanelId {
    pub fn encode(&self) -> String {
        match self {
            PanelId::Home => "mp:h".into(),
            PanelId::Category(c) => format!("mp:c:{}", c.token()),
            PanelId::Action(a) => format!("mp:a:{}", a.token()),
            PanelId::PickUser(a) => format!("mp:u:{}", a.token()),
            PanelId::PickChannel(a, u) => format!("mp:p:{}:{}", a.token(), opt(u.map(|u| u.get()))),
            PanelId::Modal(a, u, c) => format!("mp:m:{}:{}:{}", a.token(), opt(u.map(|u| u.get())), opt(c.map(|c| c.get()))),
            PanelId::Config(c) => c.encode(),
            PanelId::Verify => "mp:v".into(),
            PanelId::VerifyQuiz => "mp:vq".into(),
            PanelId::ReviewPick => "mp:rp".into(),
            PanelId::Review(approve, u) => format!("mp:rv:{}:{}", if *approve { 'a' } else { 'd' }, u.get()),
        }
    }

    pub fn decode(s: &str) -> Option<Self> {
        let parts: Vec<&str> = s.strip_prefix("mp:")?.split(':').collect();
        let user = |s| parse_opt(s).map(|o| o.map(UserId::new));
        let chan = |s| parse_opt(s).map(|o| o.map(ChannelId::new));
        match parts.as_slice() {
            ["h"] => Some(PanelId::Home),
            ["c", c] => Category::parse(c).map(PanelId::Category),
            ["a", a] => Action::parse(a).map(PanelId::Action),
            ["u", a] => Action::parse(a).map(PanelId::PickUser),
            ["p", a, u] => Some(PanelId::PickChannel(Action::parse(a)?, user(u)?)),
            ["m", a, u, c] => Some(PanelId::Modal(Action::parse(a)?, user(u)?, chan(c)?)),
            ["v"] => Some(PanelId::Verify),
            ["vq"] => Some(PanelId::VerifyQuiz),
            ["rp"] => Some(PanelId::ReviewPick),
            ["rv", k @ ("a" | "d"), u] => Some(PanelId::Review(*k == "a", user(u)??)),
            ["g", rest @ ..] => ConfigId::decode(rest).map(PanelId::Config),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_ids() -> Vec<ConfigId> {
        let mut v = vec![ConfigId::LogCat, ConfigId::DmKind, ConfigId::DecayOff, ConfigId::DmShowMod(true), ConfigId::DmShowMod(false), ConfigId::FilterSet(true), ConfigId::FilterSet(false)];
        v.extend(PAGES.iter().map(|(p, _)| ConfigId::Open(*p)));
        v.extend(ROLE_LISTS.iter().map(|(r, _)| ConfigId::Roles(*r)));
        v.extend(FORMS.iter().flat_map(|(f, _)| [ConfigId::Form(*f), ConfigId::Submit(*f)]));
        for c in LogCategory::ALL.into_iter().map(Some).chain([None]) {
            v.extend([ConfigId::LogChan(c), ConfigId::LogOff(c)]);
        }
        for k in crate::moderation::templates::dm_kinds() {
            v.push(ConfigId::DmText(*k));
        }
        v
    }

    #[test]
    fn round_trips_and_rejects_junk() {
        let big_u = UserId::new(u64::MAX);
        let big_c = ChannelId::new(u64::MAX);
        let mut all = vec![
            PanelId::Home,
            PanelId::Category(Category::Config),
            PanelId::Action(Action::NoteDelete),
            PanelId::PickUser(Action::Kick),
            PanelId::PickChannel(Action::Move, Some(big_u)),
            PanelId::PickChannel(Action::Purge, None),
            PanelId::Modal(Action::Move, Some(big_u), Some(big_c)),
            PanelId::Modal(Action::Unban, None, None),
            PanelId::Review(true, big_u),
            PanelId::Review(false, UserId::new(1)),
            PanelId::Verify,
            PanelId::VerifyQuiz,
            PanelId::ReviewPick,
            PanelId::Category(Category::Safety),
            PanelId::Action(Action::EndLockdown),
        ];
        all.extend(config_ids().into_iter().map(PanelId::Config));
        all.extend(CATEGORIES.iter().map(|(c, _)| PanelId::Category(*c)));
        all.extend(ACTIONS.iter().map(|(a, _)| PanelId::Modal(*a, Some(big_u), Some(big_c))));
        for id in all {
            let s = id.encode();
            assert!(s.starts_with("mp:") && s.len() <= 100);
            assert_eq!(PanelId::decode(&s), Some(id));
        }
        assert_eq!(PanelId::Verify.encode(), "mp:v");
        assert_eq!(PanelId::VerifyQuiz.encode(), "mp:vq");
        assert_eq!(PanelId::ReviewPick.encode(), "mp:rp");
        assert_eq!(PanelId::Category(Category::Safety).encode(), "mp:c:safety");
        assert_eq!(PanelId::Action(Action::EndLockdown).encode(), "mp:a:unlock");
        assert_eq!(PanelId::Review(true, UserId::new(7)).encode(), "mp:rv:a:7");
        assert_eq!(PanelId::Review(false, UserId::new(7)).encode(), "mp:rv:d:7");
        for junk in ["", "mp:", "mp:zz", "mp:m:kick:abc:-", "other:home", "mp:c:members:extra", "mp:m:kick:0:-", "mp:p:move:+5", "mp:g", "mp:g:o:zz", "mp:g:ds:2", "mp:g:lh:nope", "mp:g:dt:ban:x", "mp:g:dt:note", "mp:rv:x:1", "mp:rv:a:0", "mp:rv:a"] {
            assert_eq!(PanelId::decode(junk), None, "{junk}");
        }
    }
}
