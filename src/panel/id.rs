use crate::automod::rules::RuleKind;
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
    AutoMod,
    Protection,
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
    Verification,
    Quarantine,
    Lockdown,
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
    Questions,
    VerifyDesc,
    LockMsg,
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
    /// Verification method: 0 off, 1 button, 2 button + quiz.
    VerifyMode(u8),
    /// Quiz answers: true = staff check, false = auto-check.
    VerifyReview(bool),
    VerifyChannel,
    VerifyRoles,
    VerifyPost,
    QuarRole,
    QuarCreate,
    QuarChannel,
    /// Deny action: true = ban, false = kick.
    QuarDeny(bool),
    QuarSync,
    LockChannels,
}

/// Banned-words and allowed-sites pop-ups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListForm {
    WordsAdd,
    WordsRemove,
    SiteAdd,
    SiteRemove,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoId {
    /// Rule string select on the category page.
    Pick,
    /// Generic page of one rule ("Action & ignores").
    Rule(RuleKind),
    /// Action select; Timeout then opens a pop-up.
    Action(RuleKind),
    /// Timeout length pop-up submit.
    Timeout(RuleKind),
    /// Ignored-roles select, saved on select.
    Roles(RuleKind),
    /// Ignored-channels select, saved on select.
    Chans(RuleKind),
    /// Turn on/off button carrying the new value.
    Toggle(RuleKind, bool),
    /// Settings button opens the pop-up; the submit carries the other id.
    Settings(RuleKind),
    SettingsSubmit(RuleKind),
    Form(ListForm),
    Submit(ListForm),
}

/// Protection ids. Raid ids carry the raid id so a review message keeps working after the raid ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtId {
    /// End raid mode button on the raid alert.
    RaidEnd,
    /// Kick (false) or ban (true) all raiders: asks to confirm.
    RaidBulk(bool),
    /// The confirm button for `RaidBulk`.
    RaidBulkConfirm(bool),
    /// Approve multi-select on the raid-ended message: raid id, page.
    ReviewApprove(i64, usize),
    ReviewApproveAll(i64),
    ReviewDenyRest(i64),
    /// Show this page of the approve select.
    ReviewPage(i64, usize),
    /// A button on an alt or new-account card, for that user.
    Alt(AltAct, UserId),
    /// Anti-nuke alert: give the stripped roles back, for this strip id.
    NukeRestore(i64),
    /// Anti-nuke alert: ban the stripped user, for this strip id.
    NukeBan(i64),
    /// Feature page button.
    Page(ProtPage),
    /// Overview button: start (true) or end raid mode.
    RaidMode(bool),
    /// Turn on/off button carrying the new value.
    Toggle(ProtPage, bool),
    /// Raid responses multi-select.
    Responses,
    /// Raid kick/ban filter multi-select.
    Filter,
    /// Alt response select.
    AltResponse,
    /// Gate action select.
    GateAction,
    /// Anti-nuke trusted members user select.
    TrustUsers,
    /// Anti-nuke trusted roles role select.
    TrustRoles,
    /// Button that opens a pop-up.
    Form(ProtForm),
    /// Pop-up submit.
    Submit(ProtForm),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtPage {
    Raid,
    Alts,
    Gate,
    Nuke,
}

/// Protection pop-ups. Anti-nuke limits are two pop-ups (Discord allows 5 inputs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtForm {
    RaidSettings,
    AltPoints,
    /// Second alt pop-up: the young-account day count.
    AltDays,
    GateSettings,
    Limits1,
    Limits2,
}

pub const PROT_PAGES: [(ProtPage, &str); 4] = [(ProtPage::Raid, "raid"), (ProtPage::Alts, "alts"), (ProtPage::Gate, "gate"), (ProtPage::Nuke, "nuke")];

const PROT_FORMS: [(ProtForm, &str); 6] =
    [(ProtForm::RaidSettings, "raid"), (ProtForm::AltPoints, "pts"), (ProtForm::AltDays, "days"), (ProtForm::GateSettings, "gate"), (ProtForm::Limits1, "lim1"), (ProtForm::Limits2, "lim2")];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AltAct {
    Approve,
    Quarantine,
    Kick,
    Ban,
}

const ALT_ACTS: [(AltAct, &str); 4] = [(AltAct::Approve, "a"), (AltAct::Quarantine, "q"), (AltAct::Kick, "k"), (AltAct::Ban, "b")];

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
    AutoMod(AutoId),
    Protection(ProtId),
    /// Quarantine review card: approve (true) or deny, for this user.
    Review(bool, UserId),
    /// Public Verify button on the verify message.
    Verify,
    /// Quiz pop-up submit.
    VerifyQuiz,
    /// Waiting-review member select.
    ReviewPick,
}

const PAGES: [(Page, &str); 10] = [
    (Page::ModRoles, "modroles"),
    (Page::Protected, "protected"),
    (Page::Logs, "logs"),
    (Page::Dm, "dm"),
    (Page::Ladder, "ladder"),
    (Page::Decay, "decay"),
    (Page::Filter, "filter"),
    (Page::Verification, "verify"),
    (Page::Quarantine, "quar"),
    (Page::Lockdown, "lock"),
];

const FORMS: [(Form, &str); 9] = [
    (Form::LadderAdd, "ladadd"),
    (Form::LadderRemove, "ladrem"),
    (Form::DecayOn, "decayon"),
    (Form::Words, "words"),
    (Form::Hoist, "hoist"),
    (Form::Fallback, "fallback"),
    (Form::Questions, "quiz"),
    (Form::VerifyDesc, "vdesc"),
    (Form::LockMsg, "lmsg"),
];

const LIST_FORMS: [(ListForm, &str); 4] = [(ListForm::WordsAdd, "wa"), (ListForm::WordsRemove, "wr"), (ListForm::SiteAdd, "sa"), (ListForm::SiteRemove, "sr")];

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
            ConfigId::VerifyMode(m) => format!("mp:g:vm:{m}"),
            ConfigId::VerifyReview(v) => format!("mp:g:vr:{}", *v as u8),
            ConfigId::VerifyChannel => "mp:g:vc".into(),
            ConfigId::VerifyRoles => "mp:g:vl".into(),
            ConfigId::VerifyPost => "mp:g:vp".into(),
            ConfigId::QuarRole => "mp:g:qr".into(),
            ConfigId::QuarCreate => "mp:g:qn".into(),
            ConfigId::QuarChannel => "mp:g:qc".into(),
            ConfigId::QuarDeny(v) => format!("mp:g:qd:{}", *v as u8),
            ConfigId::QuarSync => "mp:g:qs".into(),
            ConfigId::LockChannels => "mp:g:lk".into(),
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
            ["vm", m @ ("0" | "1" | "2")] => m.parse().ok().map(ConfigId::VerifyMode),
            ["vr", v] => flag(v).map(ConfigId::VerifyReview),
            ["vc"] => Some(ConfigId::VerifyChannel),
            ["vl"] => Some(ConfigId::VerifyRoles),
            ["vp"] => Some(ConfigId::VerifyPost),
            ["qr"] => Some(ConfigId::QuarRole),
            ["qn"] => Some(ConfigId::QuarCreate),
            ["qc"] => Some(ConfigId::QuarChannel),
            ["qd", v] => flag(v).map(ConfigId::QuarDeny),
            ["qs"] => Some(ConfigId::QuarSync),
            ["lk"] => Some(ConfigId::LockChannels),
            _ => None,
        }
    }
}

impl AutoId {
    fn encode(&self) -> String {
        match self {
            AutoId::Pick => "mp:z:p".into(),
            AutoId::Rule(k) => format!("mp:z:o:{}", k.as_str()),
            AutoId::Action(k) => format!("mp:z:a:{}", k.as_str()),
            AutoId::Timeout(k) => format!("mp:z:t:{}", k.as_str()),
            AutoId::Roles(k) => format!("mp:z:r:{}", k.as_str()),
            AutoId::Chans(k) => format!("mp:z:c:{}", k.as_str()),
            AutoId::Toggle(k, v) => format!("mp:z:e:{}:{}", k.as_str(), *v as u8),
            AutoId::Settings(k) => format!("mp:z:n:{}", k.as_str()),
            AutoId::SettingsSubmit(k) => format!("mp:z:ns:{}", k.as_str()),
            AutoId::Form(f) => format!("mp:z:f:{}", tok(&LIST_FORMS, *f)),
            AutoId::Submit(f) => format!("mp:z:s:{}", tok(&LIST_FORMS, *f)),
        }
    }

    fn decode(parts: &[&str]) -> Option<Self> {
        let kind = |s: &str| RuleKind::from_str(s).ok();
        match parts {
            ["p"] => Some(AutoId::Pick),
            ["o", k] => kind(k).map(AutoId::Rule),
            ["a", k] => kind(k).map(AutoId::Action),
            ["t", k] => kind(k).map(AutoId::Timeout),
            ["r", k] => kind(k).map(AutoId::Roles),
            ["c", k] => kind(k).map(AutoId::Chans),
            ["e", k, v] => Some(AutoId::Toggle(kind(k)?, flag(v)?)),
            ["n", k] => kind(k).map(AutoId::Settings),
            ["ns", k] => kind(k).map(AutoId::SettingsSubmit),
            ["f", f] => untok(&LIST_FORMS, f).map(AutoId::Form),
            ["s", f] => untok(&LIST_FORMS, f).map(AutoId::Submit),
            _ => None,
        }
    }
}

/// Plain non-negative number with one encoding: no sign, no leading zero.
fn num<T: FromStr>(s: &str) -> Option<T> {
    (s == "0" || (!s.starts_with('0') && !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))).then(|| s.parse().ok()).flatten()
}

impl ProtId {
    fn encode(&self) -> String {
        match self {
            ProtId::RaidEnd => "mp:pr:re".into(),
            ProtId::RaidBulk(ban) => format!("mp:pr:rb:{}", *ban as u8),
            ProtId::RaidBulkConfirm(ban) => format!("mp:pr:rc:{}", *ban as u8),
            ProtId::ReviewApprove(r, p) => format!("mp:pr:ra:{r}:{p}"),
            ProtId::ReviewApproveAll(r) => format!("mp:pr:aa:{r}"),
            ProtId::ReviewDenyRest(r) => format!("mp:pr:dr:{r}"),
            ProtId::ReviewPage(r, p) => format!("mp:pr:rp:{r}:{p}"),
            ProtId::Alt(a, u) => format!("mp:pr:al:{}:{}", tok(&ALT_ACTS, *a), u.get()),
            ProtId::NukeRestore(s) => format!("mp:pr:nr:{s}"),
            ProtId::NukeBan(s) => format!("mp:pr:nb:{s}"),
            ProtId::Page(p) => format!("mp:pr:pg:{}", tok(&PROT_PAGES, *p)),
            ProtId::RaidMode(v) => format!("mp:pr:rm:{}", *v as u8),
            ProtId::Toggle(p, v) => format!("mp:pr:tg:{}:{}", tok(&PROT_PAGES, *p), *v as u8),
            ProtId::Responses => "mp:pr:rr".into(),
            ProtId::Filter => "mp:pr:rf".into(),
            ProtId::AltResponse => "mp:pr:ar".into(),
            ProtId::GateAction => "mp:pr:ga".into(),
            ProtId::TrustUsers => "mp:pr:tu".into(),
            ProtId::TrustRoles => "mp:pr:tr".into(),
            ProtId::Form(f) => format!("mp:pr:f:{}", tok(&PROT_FORMS, *f)),
            ProtId::Submit(f) => format!("mp:pr:s:{}", tok(&PROT_FORMS, *f)),
        }
    }

    fn decode(parts: &[&str]) -> Option<Self> {
        match parts {
            ["re"] => Some(ProtId::RaidEnd),
            ["rb", v] => flag(v).map(ProtId::RaidBulk),
            ["rc", v] => flag(v).map(ProtId::RaidBulkConfirm),
            ["ra", r, p] => Some(ProtId::ReviewApprove(num(r)?, num(p)?)),
            ["aa", r] => num(r).map(ProtId::ReviewApproveAll),
            ["dr", r] => num(r).map(ProtId::ReviewDenyRest),
            ["rp", r, p] => Some(ProtId::ReviewPage(num(r)?, num(p)?)),
            ["al", a, u] => Some(ProtId::Alt(untok(&ALT_ACTS, a)?, UserId::new(num::<u64>(u).filter(|n| *n > 0)?))),
            ["nr", s] => num(s).map(ProtId::NukeRestore),
            ["nb", s] => num(s).map(ProtId::NukeBan),
            ["pg", p] => untok(&PROT_PAGES, p).map(ProtId::Page),
            ["rm", v] => flag(v).map(ProtId::RaidMode),
            ["tg", p, v] => Some(ProtId::Toggle(untok(&PROT_PAGES, p)?, flag(v)?)),
            ["rr"] => Some(ProtId::Responses),
            ["rf"] => Some(ProtId::Filter),
            ["ar"] => Some(ProtId::AltResponse),
            ["ga"] => Some(ProtId::GateAction),
            ["tu"] => Some(ProtId::TrustUsers),
            ["tr"] => Some(ProtId::TrustRoles),
            ["f", f] => untok(&PROT_FORMS, f).map(ProtId::Form),
            ["s", f] => untok(&PROT_FORMS, f).map(ProtId::Submit),
            _ => None,
        }
    }
}

const CATEGORIES: [(Category, &str); 8] = [
    (Category::Members, "members"),
    (Category::NamesVoice, "names"),
    (Category::Channels, "channels"),
    (Category::Safety, "safety"),
    (Category::AutoMod, "automod"),
    (Category::Protection, "protection"),
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
            PanelId::AutoMod(a) => a.encode(),
            PanelId::Protection(p) => p.encode(),
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
            ["z", rest @ ..] => AutoId::decode(rest).map(PanelId::AutoMod),
            ["pr", rest @ ..] => ProtId::decode(rest).map(PanelId::Protection),
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
        v.extend([ConfigId::VerifyMode(0), ConfigId::VerifyMode(1), ConfigId::VerifyMode(2), ConfigId::VerifyReview(true), ConfigId::VerifyReview(false)]);
        v.extend([ConfigId::VerifyChannel, ConfigId::VerifyRoles, ConfigId::VerifyPost, ConfigId::QuarRole, ConfigId::QuarCreate, ConfigId::QuarChannel]);
        v.extend([ConfigId::QuarDeny(true), ConfigId::QuarDeny(false), ConfigId::QuarSync, ConfigId::LockChannels]);
        for c in LogCategory::ALL.into_iter().map(Some).chain([None]) {
            v.extend([ConfigId::LogChan(c), ConfigId::LogOff(c)]);
        }
        for k in crate::moderation::templates::dm_kinds() {
            v.push(ConfigId::DmText(*k));
        }
        v
    }

    fn prot_ids() -> Vec<ProtId> {
        let big_u = UserId::new(u64::MAX);
        let mut v = vec![
            ProtId::RaidEnd,
            ProtId::RaidBulk(true),
            ProtId::RaidBulk(false),
            ProtId::RaidBulkConfirm(true),
            ProtId::RaidBulkConfirm(false),
            ProtId::ReviewApprove(i64::MAX, usize::MAX),
            ProtId::ReviewApprove(1, 0),
            ProtId::ReviewApproveAll(i64::MAX),
            ProtId::ReviewDenyRest(7),
            ProtId::ReviewPage(i64::MAX, 3),
            ProtId::ReviewPage(1, 0),
            ProtId::NukeRestore(i64::MAX),
            ProtId::NukeRestore(1),
            ProtId::NukeBan(i64::MAX),
            ProtId::NukeBan(0),
            ProtId::RaidMode(true),
            ProtId::RaidMode(false),
            ProtId::Responses,
            ProtId::Filter,
            ProtId::AltResponse,
            ProtId::GateAction,
            ProtId::TrustUsers,
            ProtId::TrustRoles,
        ];
        v.extend(ALT_ACTS.iter().map(|(a, _)| ProtId::Alt(*a, big_u)));
        v.push(ProtId::Alt(AltAct::Quarantine, UserId::new(1)));
        v.extend(PROT_PAGES.iter().flat_map(|(p, _)| [ProtId::Page(*p), ProtId::Toggle(*p, true), ProtId::Toggle(*p, false)]));
        v.extend(PROT_FORMS.iter().flat_map(|(f, _)| [ProtId::Form(*f), ProtId::Submit(*f)]));
        v
    }

    #[test]
    fn prot_ids_round_trip() {
        for id in prot_ids().into_iter().map(PanelId::Protection) {
            let s = id.encode();
            assert!(s.starts_with("mp:pr:") && s.len() <= 100, "{s}");
            assert_eq!(PanelId::decode(&s), Some(id), "{s}");
        }
        assert_eq!(PanelId::Protection(ProtId::Toggle(ProtPage::Nuke, false)).encode(), "mp:pr:tg:nuke:0");
        assert_eq!(PanelId::Category(Category::Protection).encode(), "mp:c:protection");
        assert_eq!(PanelId::Protection(ProtId::Submit(ProtForm::AltDays)).encode(), "mp:pr:s:days");
        assert!(prot_ids().contains(&ProtId::Form(ProtForm::AltDays)));
        for junk in ["mp:pr:pg:zz", "mp:pr:pg", "mp:pr:rm:2", "mp:pr:tg:raid", "mp:pr:tg:raid:2", "mp:pr:tg:zz:1", "mp:pr:rr:x", "mp:pr:f:zz", "mp:pr:s", "mp:pr:tu:1"] {
            assert_eq!(PanelId::decode(junk), None, "{junk}");
        }
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
        all.extend(
            [AutoId::Pick]
                .into_iter()
                .chain(RuleKind::ALL.into_iter().flat_map(|k| {
                    [AutoId::Rule(k), AutoId::Action(k), AutoId::Timeout(k), AutoId::Roles(k), AutoId::Chans(k), AutoId::Toggle(k, true), AutoId::Toggle(k, false), AutoId::Settings(k), AutoId::SettingsSubmit(k)]
                }))
                .chain(LIST_FORMS.iter().flat_map(|(f, _)| [AutoId::Form(*f), AutoId::Submit(*f)]))
                .map(PanelId::AutoMod),
        );
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
        assert_eq!(PanelId::Category(Category::AutoMod).encode(), "mp:c:automod");
        assert_eq!(PanelId::AutoMod(AutoId::Toggle(RuleKind::Links, true)).encode(), "mp:z:e:links:1");
        assert_eq!(PanelId::Action(Action::EndLockdown).encode(), "mp:a:unlock");
        assert_eq!(PanelId::Review(true, UserId::new(7)).encode(), "mp:rv:a:7");
        assert_eq!(PanelId::Review(false, UserId::new(7)).encode(), "mp:rv:d:7");
        assert_eq!(PanelId::Protection(ProtId::ReviewApprove(12, 1)).encode(), "mp:pr:ra:12:1");
        assert_eq!(PanelId::Protection(ProtId::RaidBulkConfirm(true)).encode(), "mp:pr:rc:1");
        for junk in ["", "mp:", "mp:zz", "mp:m:kick:abc:-", "other:home", "mp:c:members:extra", "mp:m:kick:0:-", "mp:p:move:+5", "mp:g", "mp:g:o:zz", "mp:g:ds:2", "mp:g:vm:3", "mp:g:vr:2", "mp:g:qd:x", "mp:g:lh:nope", "mp:g:dt:ban:x", "mp:g:dt:note", "mp:rv:x:1", "mp:z", "mp:z:o:nope", "mp:z:e:words:2", "mp:z:e:words", "mp:z:f:zz", "mp:z:p:x", "mp:c:automod:x", "mp:rv:a:0", "mp:rv:a", "mp:pr", "mp:pr:re:x", "mp:pr:rb:2", "mp:pr:ra:1", "mp:pr:ra:01:0", "mp:pr:ra:1:-1", "mp:pr:rp:1:00", "mp:pr:aa:", "mp:pr:dr:+1", "mp:pr:aa:99999999999999999999", "mp:pr:al:a:0", "mp:pr:al:x:1", "mp:pr:al:a", "mp:pr:al:b:01", "mp:pr:nr", "mp:pr:nr:-1", "mp:pr:nb:01", "mp:pr:nb:1:2"] {
            assert_eq!(PanelId::decode(junk), None, "{junk}");
        }
    }
}
