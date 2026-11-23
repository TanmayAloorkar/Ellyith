//! AutoMod panel: rule picker, per-rule page, banned-words and links pages. Every save goes through a
//! `crate::automod::commands` core, which re-checks Manage Server, so a stale or forwarded panel cannot change settings.
use super::flows::{modal_values, msg, outcome_of, show_views, stale, STALE};
use super::id::{AutoId, Category, ListForm, PanelId};
use super::view;
use crate::automod::commands::{self as core, settings_used, Targets};
use crate::automod::rules::{RuleAction, RuleConfig, RuleKind};
use crate::automod::store;
use crate::duration::parse_duration;
use crate::invoker::Invoker;
use crate::moderation::perms::require_manage_server;
use crate::reply::refused;
use crate::{Data, Error, UserError};
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelId, ComponentInteraction, ComponentInteractionDataKind, CreateActionRow, CreateAllowedMentions, CreateButton, CreateEmbed, CreateInputText,
    CreateInteractionResponse, CreateInteractionResponseMessage, CreateModal, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption, EditInteractionResponse, InputTextStyle,
    MessageFlags, ModalInteraction, RoleId,
};
use std::future::Future;
use std::str::FromStr;

type Views = (CreateEmbed, Vec<CreateActionRow>);

const MAX_IGNORED: usize = 25;
const ACTIONS: [RuleAction; 5] = [RuleAction::Delete, RuleAction::DeleteWarn, RuleAction::Timeout, RuleAction::Kick, RuleAction::Ban];
/// Field names in the order `set_settings_core` takes them.
const FIELDS: [&str; 4] = ["threshold", "window", "percent", "extra"];

fn id(a: AutoId) -> String {
    PanelId::AutoMod(a).encode()
}

fn button(a: AutoId, label: &str, style: ButtonStyle) -> CreateButton {
    CreateButton::new(id(a)).label(label).style(style)
}

/// Back to the AutoMod category.
pub fn back_row() -> CreateActionRow {
    CreateActionRow::Buttons(vec![button_to_category()])
}

fn button_to_category() -> CreateButton {
    CreateButton::new(PanelId::Category(Category::AutoMod).encode()).label("← AutoMod").style(ButtonStyle::Secondary)
}

/// String select of all 13 rules.
pub fn rule_menu() -> CreateActionRow {
    let options = RuleKind::ALL.iter().map(|k| CreateSelectMenuOption::new(format!("{} {}", k.emoji(), k.label()), k.as_str())).collect();
    CreateActionRow::SelectMenu(CreateSelectMenu::new(id(AutoId::Pick), CreateSelectMenuKind::String { options }).placeholder("Choose a rule to turn on or tune"))
}

// ---- pure helpers ----

/// Joins pop-up lines with the `;` the slash-command cores split on.
/// shortcut: a regex containing `;` is split in two, use /automod words add for those.
fn join_lines(s: &str) -> String {
    s.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(";")
}

fn parse_num(s: &str, what: &str) -> Result<i32, UserError> {
    s.trim().parse().map_err(|_| UserError(format!("{what} must be a whole number.")))
}

fn threshold_label(k: RuleKind) -> &'static str {
    match k {
        RuleKind::FastMessages => "Messages that trigger it (2-100)",
        RuleKind::RepeatedMessages => "Similar messages that trigger it (2-100)",
        RuleKind::FileSpam => "Files that trigger it (2-100)",
        RuleKind::MassMentions => "Mentions that trigger it (1-100)",
        RuleKind::EmojiSpam => "Emojis that trigger it (1-100)",
        RuleKind::AllCaps => "Minimum letters (1-100)",
        RuleKind::WallsOfText => "Lines that trigger it (1-100)",
        RuleKind::SpoilerSpam => "Spoilers that trigger it (1-100)",
        RuleKind::Zalgo => "Stacked marks that trigger it (1-20)",
        _ => "Threshold",
    }
}

fn percent_label(k: RuleKind) -> &'static str {
    match k {
        RuleKind::RepeatedMessages => "How similar, percent (1-100)",
        RuleKind::AllCaps => "Percent capitals (1-100)",
        RuleKind::Zalgo => "Percent of text (1-100)",
        _ => "Percent (1-100)",
    }
}

/// Pop-up inputs for a rule as (label, current value): only the numbers it uses, in `settings_used` order.
fn settings_inputs(c: &RuleConfig) -> Vec<(&'static str, i32)> {
    settings_used(c.kind)
        .iter()
        .map(|f| match *f {
            "threshold" => (threshold_label(c.kind), c.threshold),
            "window" => ("Window in seconds (1-300)", c.window_secs),
            "percent" => (percent_label(c.kind), c.percent),
            _ => ("Characters that trigger it (1-4000)", c.extra),
        })
        .collect()
}

/// Pop-up values (in `settings_used` order) to the four optional arguments of `set_settings_core`.
fn settings_args(kind: RuleKind, v: &[String]) -> Result<[Option<i32>; 4], UserError> {
    let mut out = [None; 4];
    for (i, f) in settings_used(kind).iter().enumerate() {
        let slot = FIELDS.iter().position(|x| x == f).expect("known field");
        out[slot] = Some(parse_num(v.get(i).map_or("", |s| s.as_str()), "Each setting")?);
    }
    Ok(out)
}

// ---- pages ----

fn input(style: InputTextStyle, label: &str, idx: usize, value: Option<String>) -> CreateActionRow {
    let mut t = CreateInputText::new(style, label, idx.to_string()).required(true);
    if let Some(v) = value {
        t = t.value(v);
    }
    CreateActionRow::InputText(t)
}

fn settings_modal(c: &RuleConfig) -> CreateModal {
    let rows = settings_inputs(c).into_iter().enumerate().map(|(i, (l, v))| input(InputTextStyle::Short, l, i, Some(v.to_string()))).collect();
    let title: String = format!("{} settings", c.kind.label()).chars().take(45).collect();
    CreateModal::new(id(AutoId::SettingsSubmit(c.kind)), title).components(rows)
}

fn timeout_modal(k: RuleKind) -> CreateModal {
    let t = CreateInputText::new(InputTextStyle::Short, "Length (10m, 2h, 1d)", "0").required(true).placeholder("10m").max_length(20);
    CreateModal::new(id(AutoId::Timeout(k)), "Timeout length").components(vec![CreateActionRow::InputText(t)])
}

fn list_modal(f: ListForm) -> CreateModal {
    let (title, label, style) = match f {
        ListForm::WordsAdd => ("Add banned words", "Words, one per line", InputTextStyle::Paragraph),
        ListForm::WordsRemove => ("Remove banned words", "Words, one per line", InputTextStyle::Paragraph),
        ListForm::SiteAdd => ("Allow a site", "Site (e.g. youtube.com)", InputTextStyle::Short),
        ListForm::SiteRemove => ("Remove an allowed site", "Site to remove", InputTextStyle::Short),
    };
    let mut t = CreateInputText::new(style, label, "0").required(true).max_length(if style == InputTextStyle::Paragraph { 4000 } else { 100 });
    if matches!(f, ListForm::WordsAdd) {
        t = t.placeholder("badword\n*wild*card\n/regex/");
    }
    CreateModal::new(id(AutoId::Submit(f)), title).components(vec![CreateActionRow::InputText(t)])
}

async fn current(inv: &Invoker<'_>, kind: RuleKind) -> Result<RuleConfig, Error> {
    require_manage_server(inv).await?;
    store::get_rules(&inv.data.db, inv.guild_id.get() as i64).await?.into_iter().find(|c| c.kind == kind).ok_or_else(|| "rule missing".into())
}

/// The category with the status embed. Manage Server only.
pub async fn category(inv: &Invoker<'_>) -> Views {
    let (_, rows) = view::category(Category::AutoMod);
    match core::status_core(inv).await {
        Ok(e) => (e, rows),
        Err(err) => (outcome_of("automod category", false, Err(err)), vec![view::back_row()]),
    }
}

async fn rule_page(inv: &Invoker<'_>, kind: RuleKind) -> Result<Views, Error> {
    let c = current(inv, kind).await?;
    let embed = core::rule_view_core(inv, kind).await?;
    let actions = ACTIONS.iter().map(|a| CreateSelectMenuOption::new(a.label(), a.as_str()).default_selection(*a == c.action)).collect();
    let action_sel = CreateSelectMenu::new(id(AutoId::Action(kind)), CreateSelectMenuKind::String { options: actions }).placeholder("What it does");
    let roles: Vec<RoleId> = c.ignored_roles.iter().take(MAX_IGNORED).map(|r| RoleId::new(*r as u64)).collect();
    let role_sel = CreateSelectMenu::new(id(AutoId::Roles(kind)), CreateSelectMenuKind::Role { default_roles: Some(roles).filter(|r| !r.is_empty()) })
        .placeholder("Ignored roles")
        .min_values(0)
        .max_values(MAX_IGNORED as u8);
    let chans: Vec<ChannelId> = c.ignored_channels.iter().take(MAX_IGNORED).map(|r| ChannelId::new(*r as u64)).collect();
    let chan_sel = CreateSelectMenu::new(id(AutoId::Chans(kind)), CreateSelectMenuKind::Channel { channel_types: None, default_channels: Some(chans).filter(|r| !r.is_empty()) })
        .placeholder("Ignored channels")
        .min_values(0)
        .max_values(MAX_IGNORED as u8);
    let mut row = vec![if c.enabled { button(AutoId::Toggle(kind, false), "Turn off", ButtonStyle::Danger) } else { button(AutoId::Toggle(kind, true), "Turn on", ButtonStyle::Success) }];
    if !settings_used(kind).is_empty() {
        row.push(button(AutoId::Settings(kind), "Settings…", ButtonStyle::Secondary));
    }
    row.push(button_to_category());
    Ok((embed, vec![CreateActionRow::SelectMenu(action_sel), CreateActionRow::SelectMenu(role_sel), CreateActionRow::SelectMenu(chan_sel), CreateActionRow::Buttons(row)]))
}

async fn list_page(inv: &Invoker<'_>, kind: RuleKind) -> Result<Views, Error> {
    let (embed, add, remove, a, r) = if kind == RuleKind::BannedWords {
        (core::words_list_core(inv).await?, "➕ Add words…", "➖ Remove words…", ListForm::WordsAdd, ListForm::WordsRemove)
    } else {
        (core::sites_list_core(inv).await?, "➕ Allow site…", "➖ Remove site…", ListForm::SiteAdd, ListForm::SiteRemove)
    };
    let row = vec![
        button(AutoId::Form(a), add, ButtonStyle::Success),
        button(AutoId::Form(r), remove, ButtonStyle::Danger),
        button(AutoId::Rule(kind), "Action & ignores", ButtonStyle::Secondary),
        button_to_category(),
    ];
    Ok((embed, vec![CreateActionRow::Buttons(row)]))
}

/// Banned words and Links open their list page, every other rule its own page.
async fn page_for(inv: &Invoker<'_>, kind: RuleKind) -> Views {
    let res = match kind {
        RuleKind::BannedWords | RuleKind::Links => list_page(inv, kind).await,
        _ => rule_page(inv, kind).await,
    };
    res.unwrap_or_else(|e| (outcome_of(kind, false, Err(e)), vec![back_row()]))
}

// ---- interactions ----

fn edit_with(embed: CreateEmbed) -> EditInteractionResponse {
    EditInteractionResponse::new().embed(embed).components(vec![back_row()]).allowed_mentions(CreateAllowedMentions::new())
}

/// Acknowledge first (the save hits the database), run it, show the result.
async fn finish(ctx: &serenity::Context, ci: &ComponentInteraction, what: AutoId, work: impl Future<Output = Result<CreateEmbed, Error>>) -> Result<(), Error> {
    ci.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
    let embed = outcome_of(what, false, work.await);
    ci.edit_response(&ctx.http, edit_with(embed)).await?;
    Ok(())
}

/// Show the pop-up, or the refusal when the Manage Server check failed.
async fn open_modal(ctx: &serenity::Context, ci: &ComponentInteraction, what: AutoId, modal: Result<CreateModal, Error>, in_ephemeral: bool) -> Result<(), Error> {
    let reply = match modal {
        Ok(m) => CreateInteractionResponse::Modal(m),
        Err(e) => show_views(in_ephemeral, (outcome_of(what, false, Err(e)), vec![back_row()])),
    };
    ci.create_response(&ctx.http, reply).await?;
    Ok(())
}

pub async fn on_component(ctx: &serenity::Context, data: &Data, ci: &ComponentInteraction, aid: AutoId) -> Result<(), Error> {
    let inv = Invoker::from_interaction(ctx, data, ci.guild_id.expect("checked in handle"), &ci.user);
    let in_ephemeral = ci.message.flags.is_some_and(|f| f.contains(MessageFlags::EPHEMERAL));
    let picked = match &ci.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values.first().cloned(),
        _ => None,
    };
    match aid {
        AutoId::Pick => {
            let Some(k) = picked.and_then(|v| RuleKind::from_str(&v).ok()) else { return stale(ctx, ci).await };
            ci.create_response(&ctx.http, show_views(in_ephemeral, page_for(&inv, k).await)).await?;
        }
        AutoId::Rule(k) => {
            let v = rule_page(&inv, k).await.unwrap_or_else(|e| (outcome_of(aid, false, Err(e)), vec![back_row()]));
            ci.create_response(&ctx.http, show_views(in_ephemeral, v)).await?;
        }
        AutoId::Action(k) => {
            let Some(a) = picked.and_then(|v| RuleAction::from_str(&v).ok()) else { return stale(ctx, ci).await };
            if a == RuleAction::Timeout {
                let m = require_manage_server(&inv).await.map(|_| timeout_modal(k));
                open_modal(ctx, ci, aid, m, in_ephemeral).await?;
            } else {
                finish(ctx, ci, aid, core::set_action_core(&inv, k, a, None)).await?;
            }
        }
        AutoId::Roles(k) => {
            let ComponentInteractionDataKind::RoleSelect { values } = &ci.data.kind else { return stale(ctx, ci).await };
            finish(ctx, ci, aid, core::ignore_set_core(&inv, k, Targets::Roles(values.iter().map(|r| r.get() as i64).collect()))).await?;
        }
        AutoId::Chans(k) => {
            let ComponentInteractionDataKind::ChannelSelect { values } = &ci.data.kind else { return stale(ctx, ci).await };
            finish(ctx, ci, aid, core::ignore_set_core(&inv, k, Targets::Channels(values.iter().map(|c| c.get() as i64).collect()))).await?;
        }
        AutoId::Toggle(k, on) => finish(ctx, ci, aid, core::set_enabled_core(&inv, k, on)).await?,
        AutoId::Settings(k) => {
            let m = current(&inv, k).await.map(|c| settings_modal(&c));
            open_modal(ctx, ci, aid, m, in_ephemeral).await?;
        }
        AutoId::Form(f) => {
            let m = require_manage_server(&inv).await.map(|_| list_modal(f));
            open_modal(ctx, ci, aid, m, in_ephemeral).await?;
        }
        AutoId::Timeout(_) | AutoId::SettingsSubmit(_) | AutoId::Submit(_) => return stale(ctx, ci).await,
    }
    Ok(())
}

async fn submit(inv: &Invoker<'_>, aid: AutoId, v: &[String]) -> Result<CreateEmbed, Error> {
    let val = |i: usize| v.get(i).map_or("", |s| s.as_str());
    match aid {
        AutoId::Timeout(k) => core::set_action_core(inv, k, RuleAction::Timeout, Some(parse_duration(val(0).trim())?)).await,
        AutoId::SettingsSubmit(k) => {
            let [t, w, p, e] = settings_args(k, v)?;
            core::set_settings_core(inv, k, t, w, p, e).await
        }
        AutoId::Submit(ListForm::WordsAdd) => core::words_add_core(inv, &join_lines(val(0))).await,
        AutoId::Submit(ListForm::WordsRemove) => core::words_remove_core(inv, &join_lines(val(0))).await,
        AutoId::Submit(ListForm::SiteAdd) => core::sites_core(inv, val(0), true).await,
        AutoId::Submit(ListForm::SiteRemove) => core::sites_core(inv, val(0), false).await,
        _ => Err(UserError(STALE.into()).into()),
    }
}

pub async fn on_modal(ctx: &serenity::Context, data: &Data, mi: &ModalInteraction, aid: AutoId) -> Result<(), Error> {
    if !matches!(aid, AutoId::Timeout(_) | AutoId::SettingsSubmit(_) | AutoId::Submit(_)) {
        mi.create_response(&ctx.http, CreateInteractionResponse::Message(msg(refused(STALE, false), vec![]).ephemeral(true))).await?;
        return Ok(());
    }
    mi.create_response(&ctx.http, CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true))).await?;
    let inv = Invoker::from_interaction(ctx, data, mi.guild_id.expect("checked in handle"), &mi.user);
    let embed = outcome_of(aid, false, submit(&inv, aid, &modal_values(mi)).await);
    mi.edit_response(&ctx.http, edit_with(embed)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_join_with_semicolons() {
        assert_eq!(join_lines(" a \n\n*b*\r\n/c/ \n"), "a;*b*;/c/");
        assert_eq!(join_lines("  \n "), "");
    }

    #[test]
    fn settings_inputs_follow_the_rule() {
        for k in RuleKind::ALL {
            let c = k.defaults();
            let i = settings_inputs(&c);
            assert_eq!(i.len(), settings_used(k).len(), "{k:?}");
            assert!(i.len() <= 5 && i.iter().all(|(l, _)| l.chars().count() <= 45), "{k:?}");
            if i.is_empty() {
                continue;
            }
            let v = serde_json::to_value(settings_modal(&c)).unwrap();
            assert!(v["title"].as_str().unwrap().chars().count() <= 45 && v["custom_id"].as_str().unwrap().len() <= 100);
            assert_eq!(v["components"].as_array().unwrap().len(), i.len());
        }
        let w = RuleKind::WallsOfText.defaults();
        let i = settings_inputs(&w);
        assert_eq!(i[0], ("Lines that trigger it (1-100)", w.threshold));
        assert_eq!(i[1], ("Characters that trigger it (1-4000)", w.extra));
        assert!(settings_inputs(&RuleKind::Links.defaults()).is_empty());
    }

    #[test]
    fn settings_values_map_to_fields() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(settings_args(RuleKind::RepeatedMessages, &v(&["4", " 30 ", "90"])).unwrap(), [Some(4), Some(30), Some(90), None]);
        assert_eq!(settings_args(RuleKind::WallsOfText, &v(&["12", "1500"])).unwrap(), [Some(12), None, None, Some(1500)]);
        assert!(settings_args(RuleKind::Zalgo, &v(&["x", "5"])).is_err());
        assert!(settings_args(RuleKind::Zalgo, &v(&["3"])).is_err());
    }

    #[test]
    fn popups_fit_discord() {
        let mut modals = vec![timeout_modal(RuleKind::FastMessages)];
        modals.extend([ListForm::WordsAdd, ListForm::WordsRemove, ListForm::SiteAdd, ListForm::SiteRemove].map(list_modal));
        for m in modals {
            let v = serde_json::to_value(m).unwrap();
            assert!(v["title"].as_str().unwrap().chars().count() <= 45 && v["custom_id"].as_str().unwrap().len() <= 100);
            let rows = v["components"].as_array().unwrap();
            assert!((1..=5).contains(&rows.len()));
            assert!(rows.iter().all(|r| r["components"][0]["label"].as_str().unwrap().chars().count() <= 45));
        }
    }

    #[test]
    fn rule_buttons_decode() {
        for b in [back_row(), rule_menu()] {
            let ids = view::button_ids(&[b]);
            assert!(ids.iter().all(|i| PanelId::decode(i).is_some()));
        }
    }
}
