//! Protection buttons. Raid alert, raid-ended review and alt card presses are mod-only, checked per press like review cards.
//! Anti-nuke alert buttons are owner-or-trusted only.
//! The Protection category (overview and four feature pages) saves only through the
//! `crate::protection::commands` cores, which re-check Manage Server or the owner.
use super::automod::{finish, input, parse_num};
use super::flows::{modal_values, msg, outcome_of, show_views, stale, STALE};
use super::id::{AltAct, Category, PanelId, ProtForm, ProtId, ProtPage};
use super::view;
use crate::invoker::Invoker;
use crate::moderation::actions::{execute, ActionRequest};
use crate::moderation::cases::{Actor, CaseKind};
use crate::moderation::perms::{authorize, is_absent, require_manage_server};
use crate::protection::commands::{self as core, NukeLimits, Points, ALT_RESPONSES, RAID_FILTERS, RAID_RESPONSES};
use crate::protection::store::{self, ProtectionSettings, RaidRow, TrustKind};
use crate::protection::{alts, antinuke, raid, EMOJI_ALTS, EMOJI_GATE, EMOJI_NUKE, EMOJI_PROTECTION, EMOJI_RAID};
use crate::reply::{self, refused, Tone};
use crate::safety::quarantine::{self, deny, release_core, QuarantineReason};
use crate::{Data, Error, UserError};
use chrono::{DateTime, Utc};
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ComponentInteraction, ComponentInteractionDataKind, CreateActionRow, CreateAllowedMentions, CreateButton, CreateEmbed,
    CreateEmbedFooter, CreateInteractionResponse, CreateInteractionResponseFollowup, CreateInteractionResponseMessage, CreateModal, CreateSelectMenu, CreateSelectMenuKind,
    CreateSelectMenuOption, EditInteractionResponse, EditMessage, InputTextStyle, MessageFlags, ModalInteraction, Permissions, RoleId, UserId,
};
use std::future::Future;
use std::time::Duration;

type Views = (CreateEmbed, Vec<CreateActionRow>);

/// Discord's select limit; the trusted selects show at most this many.
const MAX_PICK: usize = 25;
const RESPONSE_LABELS: [(&str, &str); 5] =
    [("alert", "Alert staff"), ("quarantine", "Quarantine joiners"), ("lockdown", "Auto lockdown"), ("kick", "Kick joiners"), ("ban", "Ban joiners")];
const FILTER_LABELS: [(&str, &str); 2] = [("age", "Account age"), ("avatar", "Default avatar")];
/// Gate actions; the first `ALT_RESPONSES.len()` are the alt responses.
const ACTION_LABELS: [(&str, &str); 4] = [("flag", "Flag to staff"), ("quarantine", "Quarantine"), ("kick", "Kick"), ("ban", "Ban")];

fn label<'a>(table: &[(&'a str, &'a str)], key: &'a str) -> &'a str {
    table.iter().find(|(k, _)| *k == key).map_or(key, |(_, l)| l)
}

fn pid(p: ProtId) -> String {
    PanelId::Protection(p).encode()
}

fn button(p: ProtId, label: &str, style: ButtonStyle) -> CreateButton {
    CreateButton::new(pid(p)).label(label).style(style)
}

fn to_overview() -> CreateButton {
    CreateButton::new(PanelId::Category(Category::Protection).encode()).label("← Protection").style(ButtonStyle::Secondary)
}

fn back_row() -> CreateActionRow {
    CreateActionRow::Buttons(vec![to_overview()])
}

fn onoff(on: bool) -> &'static str {
    if on { "On" } else { "Off" }
}

fn plural(n: i32, unit: &str) -> String {
    format!("{n} {unit}{}", if n == 1 { "" } else { "s" })
}

fn lines<K: AsRef<str>>(l: &[(K, String)]) -> String {
    l.iter().map(|(k, v)| format!("**{}** {v}", k.as_ref())).collect::<Vec<_>>().join("\n")
}

fn embed(title: String, desc: &str) -> CreateEmbed {
    CreateEmbed::new().colour(Tone::Safety.colour()).title(title).description(desc)
}

// ---- pages (pure: settings in, embed and rows out) ----

/// Overview buttons; Start raid mode becomes End raid mode while a raid is active.
pub fn overview_rows(active: bool) -> Vec<CreateActionRow> {
    let pages = [
        (ProtPage::Raid, format!("{EMOJI_RAID} Raid")),
        (ProtPage::Alts, format!("{EMOJI_ALTS} Alts")),
        (ProtPage::Gate, format!("{EMOJI_GATE} New accounts")),
        (ProtPage::Nuke, format!("{EMOJI_NUKE} Anti-nuke")),
    ];
    let mode = if active {
        button(ProtId::RaidMode(false), "✅ End raid mode", ButtonStyle::Success)
    } else {
        button(ProtId::RaidMode(true), &format!("{EMOJI_PROTECTION} Start raid mode"), ButtonStyle::Danger)
    };
    vec![
        CreateActionRow::Buttons(pages.iter().map(|(p, l)| button(ProtId::Page(*p), l, ButtonStyle::Primary)).collect()),
        CreateActionRow::Buttons(vec![mode, CreateButton::new(PanelId::Home.encode()).label("← Back").style(ButtonStyle::Secondary)]),
    ]
}

fn overview(s: &ProtectionSettings, trusted: usize, active: Option<&RaidRow>, last: Option<DateTime<Utc>>) -> Views {
    let status = match (active, last) {
        (Some(r), _) => format!("Raid mode is **active**. Started <t:{}:R>, ends <t:{}:R>.", r.started_at.timestamp(), r.ends_at.timestamp()),
        (None, Some(t)) => format!("Raid mode is not active. Last raid: <t:{}:R>.", t.timestamp()),
        (None, None) => "Raid mode is not active. Last raid: none.".into(),
    };
    let on = |enabled: bool, rest: String| if enabled { format!("On · {rest}") } else { "Off".into() };
    let responses = if s.raid_responses.is_empty() { "no responses".into() } else { s.raid_responses.join(" + ") };
    let alt = label(&ACTION_LABELS, &s.alt_response).to_lowercase();
    let gate = label(&ACTION_LABELS, &s.gate_action).to_lowercase();
    let l = [
        (format!("{EMOJI_RAID} Raid protection"), on(s.raid_enabled, format!("{} joins in {}s · {responses}", s.raid_joins, s.raid_secs))),
        (format!("{EMOJI_ALTS} Alt detection"), on(s.alt_enabled, format!("score {}+ · {alt}", s.alt_threshold))),
        (format!("{EMOJI_GATE} New account gate"), on(s.gate_enabled, format!("under {} · {gate}", plural(s.gate_days, "day")))),
        (format!("{EMOJI_NUKE} Anti-nuke"), on(s.nuke_enabled, format!("7 limits · {trusted} trusted"))),
    ];
    (embed(format!("{EMOJI_PROTECTION} Protection"), &format!("{status}\n\n{}", lines(&l))), overview_rows(active.is_some()))
}

fn page_buttons(p: ProtPage, on: bool, form: ProtForm, form_label: &str) -> CreateActionRow {
    let toggle = if on { button(ProtId::Toggle(p, false), "Turn off", ButtonStyle::Danger) } else { button(ProtId::Toggle(p, true), "Turn on", ButtonStyle::Success) };
    CreateActionRow::Buttons(vec![toggle, button(ProtId::Form(form), form_label, ButtonStyle::Secondary), to_overview()])
}

/// String select whose defaults are the current values.
fn string_select(id: ProtId, table: &[(&str, &str)], current: &[String], placeholder: &str, multi: bool) -> CreateActionRow {
    let options = table.iter().map(|(k, l)| CreateSelectMenuOption::new(*l, *k).default_selection(current.iter().any(|c| c == k))).collect();
    let mut m = CreateSelectMenu::new(pid(id), CreateSelectMenuKind::String { options }).placeholder(placeholder);
    if multi {
        m = m.min_values(0).max_values(table.len() as u8);
    }
    CreateActionRow::SelectMenu(m)
}

fn raid_page(s: &ProtectionSettings) -> Views {
    let filter = if s.raid_filter.is_empty() {
        "always (no filter)".to_string()
    } else {
        s.raid_filter
            .iter()
            .map(|f| if f == "age" { format!("account under {} old", plural(s.raid_filter_days, "day")) } else { "default avatar".into() })
            .collect::<Vec<_>>()
            .join(", or ")
    };
    let responses = if s.raid_responses.is_empty() { "none".into() } else { s.raid_responses.iter().map(|r| label(&RESPONSE_LABELS, r)).collect::<Vec<_>>().join(" · ") };
    let l = [
        ("Trigger", format!("{} joins in {}", s.raid_joins, plural(s.raid_secs, "second"))),
        ("Responses", responses),
        ("Kick/ban only if", filter),
        ("Auto-end after", format!("{} with no new burst", plural(s.raid_end_mins, "minute"))),
    ];
    let desc = "Starts raid mode when many people join at once. Everyone in that burst, and everyone who joins while raid mode is on, counts as a raider.";
    let e = embed(format!("{EMOJI_RAID} Raid protection · {}", onoff(s.raid_enabled)), &format!("{desc}\n\n{}", lines(&l)));
    let rows = vec![
        string_select(ProtId::Responses, &RESPONSE_LABELS, &s.raid_responses, "Responses", true),
        string_select(ProtId::Filter, &FILTER_LABELS, &s.raid_filter, "Kick/ban filter", true),
        page_buttons(ProtPage::Raid, s.raid_enabled, ProtForm::RaidSettings, "Settings…"),
    ];
    (e, rows)
}

fn alts_page(s: &ProtectionSettings) -> Views {
    let signals = [
        (format!("Account under {}", plural(s.alt_days, "day")), s.alt_pts_age.to_string()),
        ("Default avatar".to_string(), s.alt_pts_avatar.to_string()),
        ("Name like a banned user".to_string(), s.alt_pts_name.to_string()),
        ("Same invite soon after a ban".to_string(), s.alt_pts_invite.to_string()),
    ];
    let result = [("Threshold", s.alt_threshold.to_string()), ("Response", label(&ACTION_LABELS, &s.alt_response).to_string())];
    let e = embed(format!("{EMOJI_ALTS} Alt detection · {}", onoff(s.alt_enabled)), "Scores every new member. At or above the threshold, the bot acts.")
        .field("Signals (points)", lines(&signals), false)
        .field("\u{200b}", lines(&result), false);
    let rows = vec![
        string_select(ProtId::AltResponse, &ACTION_LABELS[..ALT_RESPONSES.len()], std::slice::from_ref(&s.alt_response), "Response", false),
        page_buttons(ProtPage::Alts, s.alt_enabled, ProtForm::AltPoints, "Points…"),
    ];
    (e, rows)
}

fn gate_page(s: &ProtectionSettings) -> Views {
    let l = [("Minimum age", plural(s.gate_days, "day")), ("Action", label(&ACTION_LABELS, &s.gate_action).to_string())];
    let desc = "Acts on every join from an account younger than the limit, raid or not.";
    let footer = format!("Kicked members get a DM: \"This server needs accounts older than {}. Come back after <date>.\"", plural(s.gate_days, "day"));
    let e = embed(format!("{EMOJI_GATE} New account gate · {}", onoff(s.gate_enabled)), &format!("{desc}\n\n{}", lines(&l))).footer(CreateEmbedFooter::new(footer));
    let rows = vec![
        string_select(ProtId::GateAction, &ACTION_LABELS, std::slice::from_ref(&s.gate_action), "Action", false),
        page_buttons(ProtPage::Gate, s.gate_enabled, ProtForm::GateSettings, "Settings…"),
    ];
    (e, rows)
}

fn trusted_of(all: &[(TrustKind, i64)], k: TrustKind) -> Vec<u64> {
    all.iter().filter(|(x, _)| *x == k).map(|(_, id)| *id as u64).collect()
}

fn nuke_page(s: &ProtectionSettings, trusted: &[(TrustKind, i64)]) -> Views {
    let limits = [
        ("Bans", s.nuke_bans.to_string()),
        ("Kicks", s.nuke_kicks.to_string()),
        ("Channel deletes", s.nuke_chan_del.to_string()),
        ("Role deletes", s.nuke_role_del.to_string()),
        ("Webhooks made", s.nuke_webhooks.to_string()),
        ("Dangerous perms given", format!("{} (Administrator, Ban, Manage Server…)", s.nuke_perms)),
        ("Bots added", "trusted only; anyone else's bot gets kicked".into()),
    ];
    let users = trusted_of(trusted, TrustKind::User);
    let roles = trusted_of(trusted, TrustKind::Role);
    let show = |ids: &[u64], p: &str| if ids.is_empty() { "none".to_string() } else { ids.iter().map(|id| format!("<{p}{id}>")).collect::<Vec<_>>().join(", ") };
    let who = [("Always", "Server owner, this bot".to_string()), ("Members", show(&users, "@")), ("Roles", show(&roles, "@&"))];
    let desc = "Stops a hacked or rogue staff account or bot. Over a limit, the bot removes all their roles and alerts you.";
    // Field values stop at 1024 characters.
    let who: String = lines(&who).chars().take(1024).collect();
    let e = embed(format!("{EMOJI_NUKE} Anti-nuke · {}", onoff(s.nuke_enabled)), desc)
        .field(format!("Limits (per person, in {})", plural(s.nuke_window_secs, "second")), lines(&limits), false)
        .field("Trusted (never punished)", who, false)
        .footer(CreateEmbedFooter::new("Only the server owner can change the trusted list or turn anti-nuke off."));
    let default_users: Vec<UserId> = users.iter().take(MAX_PICK).map(|u| UserId::new(*u)).collect();
    let default_roles: Vec<RoleId> = roles.iter().take(MAX_PICK).map(|r| RoleId::new(*r)).collect();
    let user_sel = CreateSelectMenu::new(pid(ProtId::TrustUsers), CreateSelectMenuKind::User { default_users: Some(default_users).filter(|v| !v.is_empty()) })
        .placeholder("Trusted members…")
        .min_values(0)
        .max_values(MAX_PICK as u8);
    let role_sel = CreateSelectMenu::new(pid(ProtId::TrustRoles), CreateSelectMenuKind::Role { default_roles: Some(default_roles).filter(|v| !v.is_empty()) })
        .placeholder("Trusted roles…")
        .min_values(0)
        .max_values(MAX_PICK as u8);
    let rows = vec![CreateActionRow::SelectMenu(user_sel), CreateActionRow::SelectMenu(role_sel), page_buttons(ProtPage::Nuke, s.nuke_enabled, ProtForm::Limits1, "Limits…")];
    (e, rows)
}

// ---- pop-ups ----

fn form_inputs(f: ProtForm, s: &ProtectionSettings) -> (&'static str, Vec<(String, i32)>) {
    let v = |l: &str, n: i32| (l.to_string(), n);
    match f {
        ProtForm::RaidSettings => (
            "Raid settings",
            vec![
                v("Joins that trigger it (2-100)", s.raid_joins),
                v("Within this many seconds (1-120)", s.raid_secs),
                v("Kick/ban filter: account age, days (0-365)", s.raid_filter_days),
                v("Auto-end after minutes (1-1440)", s.raid_end_mins),
            ],
        ),
        ProtForm::AltPoints => (
            "Alt points",
            vec![
                (format!("Account under {} (0-100)", plural(s.alt_days, "day")), s.alt_pts_age),
                v("Default avatar (0-100)", s.alt_pts_avatar),
                v("Name like a banned user (0-100)", s.alt_pts_name),
                v("Same invite soon after a ban (0-100)", s.alt_pts_invite),
                v("Threshold (1-400)", s.alt_threshold),
            ],
        ),
        ProtForm::AltDays => ("Alt detection settings", vec![v("Account age under (days)", s.alt_days)]),
        ProtForm::GateSettings => ("New account gate", vec![v("Minimum account age, days (0-365)", s.gate_days)]),
        ProtForm::Limits1 => (
            "Anti-nuke limits (1 of 2)",
            vec![v("Window in seconds (1-300)", s.nuke_window_secs), v("Bans (1-50)", s.nuke_bans), v("Kicks (1-50)", s.nuke_kicks), v("Channel deletes (1-50)", s.nuke_chan_del)],
        ),
        ProtForm::Limits2 => (
            "Anti-nuke limits (2 of 2)",
            vec![v("Role deletes (1-50)", s.nuke_role_del), v("Webhooks made (1-50)", s.nuke_webhooks), v("Dangerous perms given (1-50)", s.nuke_perms)],
        ),
    }
}

fn form_modal(f: ProtForm, s: &ProtectionSettings) -> CreateModal {
    let (title, inputs) = form_inputs(f, s);
    let rows = inputs.into_iter().enumerate().map(|(i, (l, n))| input(InputTextStyle::Short, &l, i, Some(n.to_string()))).collect();
    CreateModal::new(pid(ProtId::Submit(f)), title).components(rows)
}

/// The first `n` pop-up values as numbers.
fn numbers(v: &[String], n: usize) -> Result<Vec<i32>, UserError> {
    (0..n).map(|i| parse_num(v.get(i).map_or("", |s| s.as_str()), "Each setting")).collect()
}

async fn submit(inv: &Invoker<'_>, f: ProtForm, v: &[String]) -> Result<CreateEmbed, Error> {
    match f {
        ProtForm::RaidSettings => {
            let n = numbers(v, 4)?;
            core::raid_settings_core(inv, None, Some(n[0]), Some(n[1]), Some(n[2]), Some(n[3])).await
        }
        ProtForm::AltPoints => {
            let n = numbers(v, 5)?;
            // Check the threshold first so a refused one saves nothing.
            core::in_range(n[4], 1, 400)?;
            let _ = core::alts_points_core(inv, Points { age: Some(n[0]), avatar: Some(n[1]), name: Some(n[2]), invite: Some(n[3]) }).await?;
            core::alts_settings_core(inv, None, None, Some(n[4])).await
        }
        ProtForm::AltDays => core::alts_settings_core(inv, None, Some(numbers(v, 1)?[0]), None).await,
        ProtForm::GateSettings => core::gate_settings_core(inv, Some(numbers(v, 1)?[0]), None).await,
        ProtForm::Limits1 => {
            let n = numbers(v, 4)?;
            core::nuke_limits_core(inv, NukeLimits { window: Some(n[0]), bans: Some(n[1]), kicks: Some(n[2]), channel_deletes: Some(n[3]), ..Default::default() }).await
        }
        ProtForm::Limits2 => {
            let n = numbers(v, 3)?;
            core::nuke_limits_core(inv, NukeLimits { role_deletes: Some(n[0]), webhooks: Some(n[1]), perms: Some(n[2]), ..Default::default() }).await
        }
    }
}

// ---- category interactions ----

async fn settings(inv: &Invoker<'_>) -> Result<ProtectionSettings, Error> {
    require_manage_server(inv).await?;
    Ok(store::get_settings(&inv.data.db, inv.guild_id.get() as i64).await?)
}

async fn trusted(inv: &Invoker<'_>) -> Result<Vec<(TrustKind, i64)>, Error> {
    Ok(store::trusted(&inv.data.db, inv.guild_id.get() as i64).await?)
}

/// The overview. Manage Server only.
pub async fn category(inv: &Invoker<'_>) -> Views {
    let res: Result<Views, Error> = async {
        let s = settings(inv).await?;
        let g = inv.guild_id.get() as i64;
        let active = store::active_raid(&inv.data.db, g).await?;
        let last = store::last_raid_end(&inv.data.db, g).await?;
        Ok(overview(&s, trusted(inv).await?.len(), active.as_ref(), last))
    }
    .await;
    res.unwrap_or_else(|e| (outcome_of("protection category", false, Err(e)), vec![view::back_row()]))
}

async fn page(inv: &Invoker<'_>, p: ProtPage) -> Result<Views, Error> {
    let s = settings(inv).await?;
    Ok(match p {
        ProtPage::Raid => raid_page(&s),
        ProtPage::Alts => alts_page(&s),
        ProtPage::Gate => gate_page(&s),
        ProtPage::Nuke => nuke_page(&s, &trusted(inv).await?),
    })
}

async fn toggle(inv: &Invoker<'_>, p: ProtPage, on: bool) -> Result<CreateEmbed, Error> {
    match p {
        ProtPage::Raid => core::raid_settings_core(inv, Some(on), None, None, None, None).await,
        ProtPage::Alts => core::alts_settings_core(inv, Some(on), None, None).await,
        ProtPage::Gate => core::gate_enabled_core(inv, on).await,
        ProtPage::Nuke => core::nuke_enabled_core(inv, on).await,
    }
}

/// Multi-select: every option goes through the add/remove core; the last reply shows the result.
/// shortcut: one save per option, not atomic; a DB error midway leaves a partial change, pressing again repairs it. Add a set-all core if that matters.
async fn set_list(inv: &Invoker<'_>, all: &[&str], picked: &[String], filter: bool) -> Result<CreateEmbed, Error> {
    let mut last = None;
    for o in all {
        let add = picked.iter().any(|p| p == o);
        last = Some(if filter { core::raid_filter_core(inv, o, add).await? } else { core::raid_responses_core(inv, o, add).await? });
    }
    Ok(last.expect("options are not empty"))
}

/// Trusted select: trust the newly picked, untrust the unpicked the select showed, then show the list.
/// shortcut: only the first 25 of each kind are manageable here; beyond that use /antinuke trust remove.
async fn set_trusted(inv: &Invoker<'_>, kind: TrustKind, picked: Vec<u64>) -> Result<CreateEmbed, Error> {
    let shown: Vec<u64> = trusted_of(&trusted(inv).await?, kind).into_iter().take(MAX_PICK).collect();
    let add = picked.iter().filter(|p| !shown.contains(p)).map(|p| (*p, true));
    let remove = shown.iter().filter(|s| !picked.contains(s)).map(|s| (*s, false));
    for (id, add) in add.chain(remove) {
        let _ = core::trust_core(inv, kind, id as i64, add).await?;
    }
    core::trust_list_core(inv).await
}

/// Acknowledge, run the save, show the reply with ← Protection.
async fn finish_prot(ctx: &serenity::Context, ci: &ComponentInteraction, id: ProtId, work: impl Future<Output = Result<CreateEmbed, Error>>) -> Result<(), Error> {
    finish(ctx, ci, PanelId::Protection(id), work, back_row()).await
}

fn is_category_id(id: ProtId) -> bool {
    matches!(
        id,
        ProtId::Page(_)
            | ProtId::Form(_)
            | ProtId::Submit(_)
            | ProtId::RaidMode(_)
            | ProtId::Toggle(..)
            | ProtId::Responses
            | ProtId::Filter
            | ProtId::AltResponse
            | ProtId::GateAction
            | ProtId::TrustUsers
            | ProtId::TrustRoles
    )
}

async fn on_category(ctx: &serenity::Context, data: &Data, ci: &ComponentInteraction, id: ProtId) -> Result<(), Error> {
    let inv = Invoker::from_interaction(ctx, data, ci.guild_id.expect("checked in handle"), &ci.user);
    let in_ephemeral = ci.message.flags.is_some_and(|f| f.contains(MessageFlags::EPHEMERAL));
    let strings = match &ci.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => Some(values.clone()),
        _ => None,
    };
    match id {
        ProtId::Page(p) => {
            let v = page(&inv, p).await.unwrap_or_else(|e| (outcome_of(id, false, Err(e)), vec![back_row()]));
            ci.create_response(&ctx.http, show_views(in_ephemeral, v)).await?;
        }
        ProtId::Form(f) => {
            let reply = match settings(&inv).await {
                Ok(s) => CreateInteractionResponse::Modal(form_modal(f, &s)),
                Err(e) => show_views(in_ephemeral, (outcome_of(id, false, Err(e)), vec![back_row()])),
            };
            ci.create_response(&ctx.http, reply).await?;
        }
        ProtId::RaidMode(true) => finish_prot(ctx, ci, id, core::raid_start_core(&inv)).await?,
        ProtId::RaidMode(false) => finish_prot(ctx, ci, id, core::raid_end_core(&inv)).await?,
        ProtId::Toggle(p, on) => finish_prot(ctx, ci, id, toggle(&inv, p, on)).await?,
        ProtId::Responses | ProtId::Filter => {
            let Some(v) = strings else { return stale(ctx, ci).await };
            let filter = id == ProtId::Filter;
            let all: &[&str] = if filter { &RAID_FILTERS } else { &RAID_RESPONSES };
            finish_prot(ctx, ci, id, set_list(&inv, all, &v, filter)).await?;
        }
        ProtId::AltResponse => {
            let Some(v) = strings.and_then(|v| v.into_iter().next()) else { return stale(ctx, ci).await };
            finish_prot(ctx, ci, id, core::alts_response_core(&inv, &v)).await?;
        }
        ProtId::GateAction => {
            let Some(v) = strings.and_then(|v| v.into_iter().next()) else { return stale(ctx, ci).await };
            finish_prot(ctx, ci, id, core::gate_settings_core(&inv, None, Some(&v))).await?;
        }
        ProtId::TrustUsers => {
            let ComponentInteractionDataKind::UserSelect { values } = &ci.data.kind else { return stale(ctx, ci).await };
            finish_prot(ctx, ci, id, set_trusted(&inv, TrustKind::User, values.iter().map(|u| u.get()).collect())).await?;
        }
        ProtId::TrustRoles => {
            let ComponentInteractionDataKind::RoleSelect { values } = &ci.data.kind else { return stale(ctx, ci).await };
            finish_prot(ctx, ci, id, set_trusted(&inv, TrustKind::Role, values.iter().map(|r| r.get()).collect())).await?;
        }
        _ => return stale(ctx, ci).await,
    }
    Ok(())
}

/// After the first limits or alt pop-up, offer the second.
fn after_submit_row(f: ProtForm) -> CreateActionRow {
    let mut row = vec![];
    match f {
        ProtForm::Limits1 => row.push(button(ProtId::Form(ProtForm::Limits2), "More limits…", ButtonStyle::Secondary)),
        ProtForm::AltPoints => row.push(button(ProtId::Form(ProtForm::AltDays), "More settings…", ButtonStyle::Secondary)),
        _ => {}
    }
    row.push(to_overview());
    CreateActionRow::Buttons(row)
}

pub async fn on_modal(ctx: &serenity::Context, data: &Data, mi: &ModalInteraction, id: ProtId) -> Result<(), Error> {
    let ProtId::Submit(f) = id else {
        mi.create_response(&ctx.http, CreateInteractionResponse::Message(msg(refused(STALE, false), vec![]).ephemeral(true))).await?;
        return Ok(());
    };
    mi.create_response(&ctx.http, CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true))).await?;
    let inv = Invoker::from_interaction(ctx, data, mi.guild_id.expect("checked in handle"), &mi.user);
    let res = submit(&inv, f, &modal_values(mi)).await;
    let row = if res.is_ok() { after_submit_row(f) } else { back_row() };
    let embed = outcome_of(id, false, res);
    mi.edit_response(&ctx.http, EditInteractionResponse::new().embed(embed).components(vec![row]).allowed_mentions(CreateAllowedMentions::new())).await?;
    Ok(())
}

// ---- alert, card and review buttons ----

fn confirm_text(ban: bool) -> (&'static str, &'static str) {
    if ban {
        ("🔨 Ban all raiders?", "Every raider still in the server is banned, one per second. This can't be undone in one step.")
    } else {
        ("👢 Kick all raiders?", "Every raider still in the server is kicked, one per second.")
    }
}

fn perm(ban: bool) -> Permissions {
    if ban { Permissions::BAN_MEMBERS } else { Permissions::KICK_MEMBERS }
}

async fn followup(ctx: &serenity::Context, ci: &ComponentInteraction, embed: CreateEmbed, rows: Vec<CreateActionRow>) -> Result<(), Error> {
    let f = CreateInteractionResponseFollowup::new().embed(embed).components(rows).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    ci.create_followup(&ctx.http, f).await?;
    Ok(())
}

/// Approve or deny each user; one summary embed with the ones that failed.
async fn review_each(inv: &Invoker<'_>, users: &[UserId], approve: bool) -> CreateEmbed {
    let (mut ok, mut failed) = (0, vec![]);
    for (i, &u) in users.iter().enumerate() {
        // Deny kicks or bans: spaced like the other bulk raid actions.
        if !approve && i > 0 {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let res = if approve { release_core(inv, u).await } else { deny(inv, u).await };
        match res {
            Ok(_) => ok += 1,
            Err(e) => {
                let why = e.downcast_ref::<UserError>().map_or_else(|| "error, see the log".to_string(), |u| u.0.clone());
                if !e.is::<UserError>() {
                    tracing::warn!("raid review of {u} failed: {e:?}");
                }
                failed.push(format!("<@{u}>: {why}"));
            }
        }
    }
    let title = if approve { format!("✅ Approved {ok} raiders") } else { format!("⛔ Denied {ok} raiders") };
    let lines: Vec<(&str, String)> = if failed.is_empty() { vec![] } else { vec![("Couldn't", failed.join("\n"))] };
    reply::done(Tone::Safety, &title, &lines, None)
}

/// Rebuild the review message's select and buttons from who is still waiting.
async fn refresh(ctx: &serenity::Context, data: &Data, ci: &ComponentInteraction, raid_id: i64, page: usize) -> Result<(), Error> {
    let guild = ci.guild_id.expect("checked in handle");
    let ids: Vec<u64> = raid::waiting(data, guild, raid_id).await?.iter().map(|r| r.user_id as u64).collect();
    ci.channel_id.edit_message(ctx, ci.message.id, EditMessage::new().components(raid::review_rows(ctx, guild, raid_id, &ids, page))).await?;
    Ok(())
}

/// A button on an alt or new-account card. The card loses its buttons and gets a footer saying who did what.
async fn alt_press(ctx: &serenity::Context, data: &Data, ci: &ComponentInteraction, inv: &Invoker<'_>, act: AltAct, user: UserId) -> Result<CreateEmbed, Error> {
    let guild = inv.guild_id;
    let perm = match act {
        AltAct::Kick => Permissions::KICK_MEMBERS,
        AltAct::Ban => Permissions::BAN_MEMBERS,
        AltAct::Approve | AltAct::Quarantine => Permissions::MODERATE_MEMBERS,
    };
    authorize(inv, perm, Some(user)).await?;
    let by = Actor::Moderator(ci.user.id.get() as i64);
    let done = match act {
        AltAct::Approve => "✅ Approved",
        AltAct::Quarantine => {
            let member = guild.member(ctx, user).await.map_err(|e| if is_absent(&e) { UserError("They are no longer in the server.".into()).into() } else { Error::from(e) })?;
            quarantine::enter(ctx, data, guild, &member, QuarantineReason::Alt, by, Some(alts::REASON.into()), None).await?;
            "🚧 Quarantined"
        }
        AltAct::Kick | AltAct::Ban => {
            let kind = if act == AltAct::Ban { CaseKind::Ban } else { CaseKind::Kick };
            let req = ActionRequest { guild_id: guild, target: user, kind, actor: by, reason: Some(alts::REASON.into()), duration: None, ban_delete_days: 0, log_case: true };
            execute(ctx, data, req).await?;
            if act == AltAct::Ban { "🔨 Banned" } else { "👢 Kicked" }
        }
    };
    let footer = format!("{done} by @{} · {}", ci.user.name, chrono::Utc::now().format("%H:%M"));
    let card = ci.message.embeds.first().cloned().map(|e| CreateEmbed::from(e).footer(CreateEmbedFooter::new(footer)));
    let mut edit = EditMessage::new().components(vec![]);
    if let Some(card) = card {
        edit = edit.embed(card);
    }
    if let Err(e) = ci.channel_id.edit_message(ctx, ci.message.id, edit).await {
        tracing::warn!("marking alt card failed: {e}");
    }
    Ok(reply::done(Tone::Safety, done, &[("User", format!("<@{user}>"))], None))
}

pub async fn on_component(ctx: &serenity::Context, data: &Data, ci: &ComponentInteraction, id: ProtId) -> Result<(), Error> {
    if is_category_id(id) {
        return on_category(ctx, data, ci, id).await;
    }
    let guild = ci.guild_id.expect("checked in handle");
    let inv = Invoker::from_interaction(ctx, data, guild, &ci.user);
    let by = Actor::Moderator(ci.user.id.get() as i64);
    let what = PanelId::Protection(id);
    match id {
        // The confirm lives on an ephemeral message: answer by editing it in place.
        ProtId::RaidBulkConfirm(raid_id, ban) => {
            ci.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
            // Drop the button first so a second press can't run it twice.
            ci.edit_response(&ctx.http, EditInteractionResponse::new().embed(reply::done(Tone::Safety, "Working…", &[], Some("One raider per second."))).components(vec![])).await?;
            let res = async {
                authorize(&inv, perm(ban), None).await?;
                raid::bulk(ctx, data, guild, raid_id, !ban, by).await
            }
            .await;
            ci.edit_response(&ctx.http, EditInteractionResponse::new().embed(outcome_of(what, false, res)).components(vec![]).allowed_mentions(CreateAllowedMentions::new())).await?;
            return Ok(());
        }
        ProtId::ReviewPage(raid_id, page) => {
            ci.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
            let res = async {
                authorize(&inv, Permissions::MODERATE_MEMBERS, None).await?;
                refresh(ctx, data, ci, raid_id, page).await
            }
            .await;
            if let Err(e) = res {
                followup(ctx, ci, outcome_of(what, false, Err(e)), vec![]).await?;
            }
            return Ok(());
        }
        _ => {}
    }
    // Everything else answers with an ephemeral followup.
    ci.create_response(&ctx.http, CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true))).await?;
    let mut rows = vec![];
    let res: Result<CreateEmbed, Error> = async {
        match id {
            ProtId::RaidEnd => {
                authorize(&inv, Permissions::MODERATE_MEMBERS, None).await?;
                raid::end(ctx, data, guild, by).await
            }
            ProtId::RaidBulk(raid_id, ban) => {
                authorize(&inv, perm(ban), None).await?;
                raid::still_active(data, guild, raid_id).await?;
                let (title, body) = confirm_text(ban);
                let label = if ban { "Ban all" } else { "Kick all" };
                rows.push(CreateActionRow::Buttons(vec![CreateButton::new(PanelId::Protection(ProtId::RaidBulkConfirm(raid_id, ban)).encode()).label(label).style(ButtonStyle::Danger)]));
                Ok(reply::done(Tone::Safety, title, &[], None).description(body))
            }
            ProtId::ReviewApprove(raid_id, page) => {
                let ComponentInteractionDataKind::StringSelect { values } = &ci.data.kind else { return Err(UserError(STALE.into()).into()) };
                authorize(&inv, Permissions::MODERATE_MEMBERS, None).await?;
                let users: Vec<UserId> = values.iter().filter_map(|v| v.parse::<u64>().ok().filter(|n| *n > 0)).map(UserId::new).collect();
                let embed = review_each(&inv, &users, true).await;
                refresh(ctx, data, ci, raid_id, page).await?;
                Ok(embed)
            }
            ProtId::ReviewApproveAll(raid_id) | ProtId::ReviewDenyRest(raid_id) => {
                authorize(&inv, Permissions::MODERATE_MEMBERS, None).await?;
                let approve = matches!(id, ProtId::ReviewApproveAll(_));
                let users: Vec<UserId> = raid::waiting(data, guild, raid_id).await?.iter().map(|r| UserId::new(r.user_id as u64)).collect();
                let embed = review_each(&inv, &users, approve).await;
                refresh(ctx, data, ci, raid_id, 0).await?;
                Ok(embed)
            }
            ProtId::Alt(act, user) => alt_press(ctx, data, ci, &inv, act, user).await,
            ProtId::NukeRestore(strip) => antinuke::press(ctx, data, guild, ci.user.id, strip, false).await,
            ProtId::NukeBan(strip) => {
                let embed = antinuke::press(ctx, data, guild, ci.user.id, strip, true).await?;
                // Banned: the alert loses its buttons so it can't be pressed twice.
                if let Err(e) = ci.channel_id.edit_message(ctx, ci.message.id, EditMessage::new().components(vec![])).await {
                    tracing::warn!("clearing anti-nuke alert buttons failed: {e}");
                }
                Ok(embed)
            }
            ProtId::RaidBulkConfirm(..) | ProtId::ReviewPage(..) => unreachable!("answered above"),
            _ => unreachable!("category ids answered by on_category"),
        }
    }
    .await;
    followup(ctx, ci, outcome_of(what, false, res), rows).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> ProtectionSettings {
        ProtectionSettings {
            guild_id: 1, raid_enabled: true, raid_joins: 10, raid_secs: 10,
            raid_responses: vec!["alert".into(), "quarantine".into()], raid_filter: vec!["age".into(), "avatar".into()], raid_filter_days: 7, raid_end_mins: 10,
            alt_enabled: true, alt_days: 7, alt_pts_age: 40, alt_pts_avatar: 20, alt_pts_name: 40, alt_pts_invite: 30,
            alt_threshold: 60, alt_response: "flag".into(), gate_enabled: false, gate_days: 7, gate_action: "quarantine".into(),
            nuke_enabled: true, nuke_window_secs: 10, nuke_bans: 3, nuke_kicks: 3, nuke_chan_del: 2, nuke_role_del: 2, nuke_webhooks: 3, nuke_perms: 1,
        }
    }

    macro_rules! json {
        ($e:expr) => {
            serde_json::to_value($e).unwrap()
        };
    }

    fn all_views() -> Vec<Views> {
        let s = defaults();
        let trusted = [(TrustKind::User, 5), (TrustKind::Role, 6)];
        vec![overview(&s, 3, None, None), raid_page(&s), alts_page(&s), gate_page(&s), nuke_page(&s, &trusted)]
    }

    #[test]
    fn pages_fit_and_ids_decode() {
        for (e, rows) in all_views() {
            let v = json!(&rows);
            assert!(v.as_array().unwrap().len() <= 5);
            for row in v.as_array().unwrap() {
                for c in row["components"].as_array().unwrap() {
                    let id = c["custom_id"].as_str().unwrap();
                    assert!(id.len() <= 100 && PanelId::decode(id).is_some(), "{id}");
                    assert!(c["options"].as_array().map_or(0, |o| o.len()) <= 25);
                }
            }
            assert!(json!(&e)["description"].as_str().unwrap().chars().count() <= 4096);
        }
    }

    #[test]
    fn pages_copy_the_mockup() {
        let s = defaults();
        let o = json!(overview(&s, 3, None, None).0);
        assert_eq!(o["title"], "🚨 Protection");
        let d = o["description"].as_str().unwrap();
        assert!(d.starts_with("Raid mode is not active. Last raid: none."));
        assert!(d.contains("**🌊 Raid protection** On · 10 joins in 10s · alert + quarantine"));
        assert!(d.contains("**🕵️ Alt detection** On · score 60+ · flag to staff"));
        assert!(d.contains("**🐣 New account gate** Off"));
        let mut g = defaults();
        g.gate_enabled = true;
        g.alt_response = "quarantine".into();
        let gd = json!(overview(&g, 0, None, None).0)["description"].as_str().unwrap().to_string();
        assert!(gd.contains("**🐣 New account gate** On · under 7 days · quarantine"), "{gd}");
        assert!(gd.contains("score 60+ · quarantine"));
        assert!(d.contains("**💣 Anti-nuke** On · 7 limits · 3 trusted"));

        let r = json!(raid_page(&s));
        assert_eq!(r[0]["title"], "🌊 Raid protection · On");
        let d = r[0]["description"].as_str().unwrap();
        assert!(d.contains("**Trigger** 10 joins in 10 seconds"));
        assert!(d.contains("**Responses** Alert staff · Quarantine joiners"));
        assert!(d.contains("**Kick/ban only if** account under 7 days old, or default avatar"));
        assert!(d.contains("**Auto-end after** 10 minutes with no new burst"));
        assert_eq!(r[1][0]["components"][0]["options"].as_array().unwrap().len(), 5);
        assert_eq!(r[1][0]["components"][0]["max_values"], 5);
        assert_eq!(r[1][1]["components"][0]["options"].as_array().unwrap().len(), 2);
        let labels = |v: &serde_json::Value| v["components"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap().to_string()).collect::<Vec<_>>();
        assert_eq!(labels(&r[1][2]), ["Turn off", "Settings…", "← Protection"]);

        let a = json!(alts_page(&s));
        assert_eq!(a[0]["fields"][0]["name"], "Signals (points)");
        assert!(a[0]["fields"][0]["value"].as_str().unwrap().contains("**Same invite soon after a ban** 30"));
        assert_eq!(a[1][0]["components"][0]["options"].as_array().unwrap().len(), 2);
        assert_eq!(labels(&a[1][1]), ["Turn off", "Points…", "← Protection"]);

        let g = json!(gate_page(&s));
        assert_eq!(g[0]["title"], "🐣 New account gate · Off");
        assert!(g[0]["description"].as_str().unwrap().contains("**Action** Quarantine"));
        assert_eq!(g[1][0]["components"][0]["options"].as_array().unwrap().len(), 4);
        assert_eq!(labels(&g[1][1]), ["Turn on", "Settings…", "← Protection"]);

        let n = json!(nuke_page(&s, &[(TrustKind::User, 5)]));
        assert_eq!(n[0]["fields"][0]["name"], "Limits (per person, in 10 seconds)");
        assert!(n[0]["fields"][1]["value"].as_str().unwrap().contains("**Members** <@5>"));
        assert_eq!(n[1][0]["components"][0]["type"], 5);
        assert_eq!(n[1][1]["components"][0]["type"], 6);
        assert_eq!(labels(&n[1][2]), ["Turn off", "Limits…", "← Protection"]);
    }

    #[test]
    fn start_becomes_end_while_active() {
        let label = |active| json!(overview_rows(active))[1]["components"][0]["label"].as_str().unwrap().to_string();
        assert_eq!(label(false), "🚨 Start raid mode");
        assert_eq!(label(true), "✅ End raid mode");
    }

    #[test]
    fn popups_fit_discord() {
        let s = defaults();
        let forms = [ProtForm::RaidSettings, ProtForm::AltPoints, ProtForm::AltDays, ProtForm::GateSettings, ProtForm::Limits1, ProtForm::Limits2];
        let sizes: Vec<usize> = forms.iter().map(|f| form_inputs(*f, &s).1.len()).collect();
        assert_eq!(sizes, [4, 5, 1, 1, 4, 3]);
        assert_eq!(form_inputs(ProtForm::AltDays, &s), ("Alt detection settings", vec![("Account age under (days)".to_string(), 7)]));
        for f in forms {
            let v = json!(form_modal(f, &s));
            assert!(v["title"].as_str().unwrap().chars().count() <= 45 && v["custom_id"].as_str().unwrap().len() <= 100);
            assert_eq!(PanelId::decode(v["custom_id"].as_str().unwrap()), Some(PanelId::Protection(ProtId::Submit(f))));
            for r in v["components"].as_array().unwrap() {
                assert!(r["components"][0]["label"].as_str().unwrap().chars().count() <= 45);
            }
        }
        let mut big = defaults();
        big.alt_days = 365;
        assert!(form_inputs(ProtForm::AltPoints, &big).1[0].0.chars().count() <= 45);
    }

    #[test]
    fn popup_numbers() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(numbers(&v(&[" 4 ", "30"]), 2).unwrap(), [4, 30]);
        assert!(numbers(&v(&["x"]), 1).is_err());
        assert!(numbers(&v(&["1"]), 2).is_err());
    }

    #[test]
    fn second_limits_popup_follows_the_first() {
        let ids = view::button_ids(&[after_submit_row(ProtForm::Limits1)]);
        assert_eq!(ids[0], pid(ProtId::Form(ProtForm::Limits2)));
        assert_eq!(view::button_ids(&[after_submit_row(ProtForm::Limits2)]), [PanelId::Category(Category::Protection).encode()]);
        let alt = json!(&[after_submit_row(ProtForm::AltPoints)]);
        assert_eq!(alt[0]["components"][0]["custom_id"], pid(ProtId::Form(ProtForm::AltDays)));
        assert_eq!(alt[0]["components"][0]["label"], "More settings…");
    }
}
