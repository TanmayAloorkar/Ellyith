//! Config panel: sub-views, selects and pop-ups. Every save goes through a `config_core` function,
//! which re-checks Manage Server, so a stale or forwarded panel cannot change settings.
use super::flows::{modal_values, msg, outcome_of, show_views, stale, STALE};
use super::id::{Category, ConfigId, Form, Page, PanelId};
use super::view;
use crate::duration::parse_duration;
use crate::invoker::Invoker;
use crate::logging::{self, LogCategory};
use crate::moderation::cases::CaseKind;
use crate::moderation::commands::config_core::{self as core, describe_rule, on_off, role_mentions, RoleList};
use crate::moderation::ladder::{self, LadderAction, LadderRule};
use crate::moderation::perms::require_manage_server;
use crate::moderation::templates;
use crate::reply::{refused, Tone};
use crate::settings::{self, GuildSettings};
use crate::{Data, Error, UserError};
use poise::serenity_prelude::{
    self as serenity, ButtonStyle, ChannelType, ComponentInteraction, ComponentInteractionDataKind, CreateActionRow, CreateAllowedMentions, CreateEmbed,
    CreateInputText, CreateInteractionResponse, CreateInteractionResponseMessage, CreateModal, CreateSelectMenu, CreateSelectMenuKind, CreateSelectMenuOption,
    EditInteractionResponse, InputTextStyle, MessageFlags, ModalInteraction, RoleId,
};
use std::future::Future;
use std::str::FromStr;

type Views = (CreateEmbed, Vec<CreateActionRow>);

// ---- pure helpers ----

fn bad(msg: &str) -> UserError {
    UserError(msg.into())
}

/// Comma-separated words: trimmed, lowercased, empties dropped, repeats dropped.
fn parse_words(s: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for w in s.split(',').map(|w| w.trim().to_lowercase()).filter(|w| !w.is_empty()) {
        if !out.contains(&w) {
            out.push(w);
        }
    }
    out
}

fn parse_whole(s: &str, what: &str) -> Result<i32, UserError> {
    s.trim().parse().map_err(|_| bad(&format!("{what} must be a whole number.")))
}

fn parse_ladder_action(s: &str) -> Result<LadderAction, UserError> {
    match s.trim().to_lowercase().as_str() {
        "timeout" => Ok(LadderAction::Timeout),
        "kick" => Ok(LadderAction::Kick),
        "tempban" => Ok(LadderAction::Tempban),
        "ban" => Ok(LadderAction::Ban),
        _ => Err(bad("Action must be timeout, kick, tempban or ban.")),
    }
}

fn config_embed(title: &str, desc: String) -> CreateEmbed {
    CreateEmbed::new().colour(Tone::Config.colour()).title(title).description(desc)
}

fn cut(s: String, max: usize) -> String {
    s.chars().take(max).collect()
}

// ---- category and sub-views ----

/// The Config category with the current settings appended. Manage Server only.
pub async fn category(inv: &Invoker<'_>) -> Views {
    let (e, rows) = view::category(Category::Config);
    match core::settings_summary(inv).await {
        Ok(lines) => (e.field("Current settings", cut(lines.join("\n"), 1024), false), rows),
        Err(err) => (outcome_of("config category", false, Err(err)), vec![view::back_row()]),
    }
}

fn menu(id: ConfigId, kind: CreateSelectMenuKind, placeholder: &str) -> CreateActionRow {
    CreateActionRow::SelectMenu(CreateSelectMenu::new(PanelId::Config(id).encode(), kind).placeholder(placeholder))
}

fn buttons(items: Vec<(ConfigId, &str, ButtonStyle)>) -> CreateActionRow {
    CreateActionRow::Buttons(items.into_iter().map(|(id, l, st)| view::config_button(id, l, st)).collect())
}

fn string_menu(id: ConfigId, placeholder: &str, options: Vec<(String, String)>) -> CreateActionRow {
    let options = options.into_iter().map(|(label, value)| CreateSelectMenuOption::new(label, value)).collect();
    menu(id, CreateSelectMenuKind::String { options }, placeholder)
}

async fn page(inv: &Invoker<'_>, p: Page) -> Result<Views, Error> {
    use ButtonStyle::{Danger as D, Secondary as S, Success as G};
    require_manage_server(inv).await?;
    let db = &inv.data.db;
    let gid = inv.guild_id.get() as i64;
    let s = settings::get_or_create(db, gid).await?;
    let back = view::config_back_row();
    Ok(match p {
        Page::ModRoles | Page::Protected => {
            let (which, title, ids) = if p == Page::ModRoles {
                (RoleList::Mod, "🛡️ Mod roles", &s.mod_roles)
            } else {
                (RoleList::Protected, "🔒 Protected roles", &s.protected_roles)
            };
            let current: Vec<RoleId> = ids.iter().take(25).map(|r| RoleId::new(*r as u64)).collect();
            let kind = CreateSelectMenuKind::Role { default_roles: Some(current).filter(|c| !c.is_empty()) };
            let sel = CreateSelectMenu::new(PanelId::Config(ConfigId::Roles(which)).encode(), kind).placeholder("Choose roles").min_values(0).max_values(25);
            let desc = format!("Current: {}\n\nPick the roles below. The list is saved as soon as you choose; clear it to empty the list.", role_mentions(ids));
            (config_embed(title, desc), vec![CreateActionRow::SelectMenu(sel), back])
        }
        Page::Logs => {
            let set = logging::list(db, gid).await?;
            let lines: Vec<String> = LogCategory::ALL
                .iter()
                .map(|c| format!("**{}** {}", c.as_str(), set.iter().find(|(x, _)| x == c).map_or("not set".to_string(), |(_, ch)| format!("<#{ch}>"))))
                .collect();
            let options = LogCategory::ALL.iter().map(|c| (c.as_str().to_string(), c.as_str().to_string())).collect();
            let desc = format!("{}\n\nPick a log category to set its channel or turn it off.", lines.join("\n"));
            (config_embed("📋 Log channels", desc), vec![string_menu(ConfigId::LogCat, "Choose a log category", options), back])
        }
        Page::Dm => {
            let options = templates::dm_kinds().iter().map(|k| (k.label().to_string(), k.as_str().to_string())).collect();
            let toggle = if s.dm_show_moderator { ("🙈 Hide moderator name", false) } else { ("👤 Show moderator name", true) };
            let desc = format!("Moderator name in DMs: **{}**\n\nPick a message to edit it. Clear the text to restore the default.", on_off(s.dm_show_moderator));
            (
                config_embed("✉️ DM messages", desc),
                vec![string_menu(ConfigId::DmKind, "Choose a message to edit", options), buttons(vec![(ConfigId::DmShowMod(toggle.1), toggle.0, S)]), back],
            )
        }
        Page::Ladder => {
            let rules = ladder::list_rules(db, gid).await?;
            let body = if rules.is_empty() {
                "No rules.".to_string()
            } else {
                rules.iter().map(|r| format!("**{} warnings** {}", r.warn_count, describe_rule(r))).collect::<Vec<_>>().join("\n")
            };
            let rows = vec![buttons(vec![(ConfigId::Form(Form::LadderAdd), "➕ Add or change rule", S), (ConfigId::Form(Form::LadderRemove), "➖ Remove rule", D)]), back];
            (config_embed("📈 Ladder", format!("{body}\n\nA rule fires when a member reaches exactly that many warnings.")), rows)
        }
        Page::Decay => {
            let now = if s.warn_decay_enabled { format!("warnings expire after {} days", s.warn_decay_days) } else { "off".into() };
            let rows = vec![buttons(vec![(ConfigId::Form(Form::DecayOn), "⏱️ Turn on…", G), (ConfigId::DecayOff, "Turn off", D)]), back];
            (config_embed("⏱️ Warn decay", format!("Now: **{now}**")), rows)
        }
        Page::Filter => {
            let words = if s.filtered_words.is_empty() { "none".into() } else { s.filtered_words.join(", ") };
            let desc = format!(
                "Filter: **{}**\n**Words** {}\n**Hoisting characters** {}\n**Fallback nickname** {}",
                on_off(s.nick_filter_enabled),
                cut(words, 600),
                s.hoist_chars,
                s.nick_fallback
            );
            let rows = vec![
                buttons(vec![
                    (ConfigId::FilterSet(true), "On", G),
                    (ConfigId::FilterSet(false), "Off", D),
                    (ConfigId::Form(Form::Words), "Words…", S),
                    (ConfigId::Form(Form::Hoist), "Hoisting characters…", S),
                    (ConfigId::Form(Form::Fallback), "Fallback name…", S),
                ]),
                back,
            ];
            (config_embed("🚫 Name filter", desc), rows)
        }
    })
}

fn log_prompt(cat: LogCategory) -> Views {
    let sel = menu(
        ConfigId::LogChan(cat),
        CreateSelectMenuKind::Channel { channel_types: Some(vec![ChannelType::Text, ChannelType::News]), default_channels: None },
        "Choose a channel",
    );
    let off = buttons(vec![(ConfigId::LogOff(cat), "Turn off", ButtonStyle::Danger)]);
    (config_embed(&format!("📋 {} logs", cat.as_str()), "Pick the channel for this log, or turn it off.".into()), vec![sel, off, view::config_back_row()])
}

// ---- pop-ups ----

fn input(label: &str, style: InputTextStyle, id: &str, required: bool, value: &str, max: u16) -> CreateActionRow {
    let shown: String = value.chars().take(max as usize).collect();
    let mut t = CreateInputText::new(style, label, id).required(required).max_length(max);
    if !shown.is_empty() {
        t = t.value(shown);
    }
    CreateActionRow::InputText(t)
}

/// Pop-up title and inputs for a form, pre-filled from the current settings.
// shortcut: a word list over 4000 characters is cut when pre-filled, so saving it unchanged would drop the tail; the slash commands still edit it.
fn form_inputs(f: Form, s: &GuildSettings) -> (&'static str, Vec<CreateActionRow>) {
    use InputTextStyle::{Paragraph, Short};
    match f {
        Form::LadderAdd => (
            "Add ladder rule",
            vec![
                input("Warnings", Short, "0", true, "", 4),
                input("Action (timeout, kick, tempban, ban)", Short, "1", true, "", 10),
                input("Duration (timeout and tempban)", Short, "2", false, "", 20),
            ],
        ),
        Form::LadderRemove => ("Remove ladder rule", vec![input("Warnings", Short, "0", true, "", 4)]),
        Form::DecayOn => ("Warn decay", vec![input("Days (1-365)", Short, "0", true, &s.warn_decay_days.to_string(), 3)]),
        Form::Words => ("Filtered words", vec![input("Words, comma-separated", Paragraph, "0", false, &s.filtered_words.join(", "), 4000)]),
        Form::Hoist => ("Hoisting characters", vec![input("Characters", Short, "0", true, &s.hoist_chars, 32)]),
        Form::Fallback => ("Fallback nickname", vec![input("Fallback nickname", Short, "0", true, &s.nick_fallback, 32)]),
    }
}

fn dm_modal(kind: CaseKind, current: &str) -> CreateModal {
    let row = input("Message (clear to restore the default)", InputTextStyle::Paragraph, "0", false, current, 1500);
    let title: String = format!("Message for {}", kind.label()).chars().take(45).collect();
    CreateModal::new(PanelId::Config(ConfigId::DmText(kind)).encode(), title).components(vec![row])
}

async fn form_modal(inv: &Invoker<'_>, f: Form) -> Result<CreateModal, Error> {
    require_manage_server(inv).await?;
    let s = settings::get_or_create(&inv.data.db, inv.guild_id.get() as i64).await?;
    let (title, rows) = form_inputs(f, &s);
    Ok(CreateModal::new(PanelId::Config(ConfigId::Submit(f)).encode(), title).components(rows))
}

async fn dm_kind_modal(inv: &Invoker<'_>, kind: CaseKind) -> Result<CreateModal, Error> {
    require_manage_server(inv).await?;
    if !templates::dm_kinds().contains(&kind) {
        return Err(bad("That action has no DM message.").into());
    }
    let text = match templates::get(&inv.data.db, inv.guild_id.get() as i64, kind).await? {
        Some(t) => t,
        None => templates::default_template(kind).to_string(),
    };
    Ok(dm_modal(kind, &text))
}

// ---- interactions ----

fn edit_with(embed: CreateEmbed) -> EditInteractionResponse {
    EditInteractionResponse::new().embed(embed).components(vec![view::config_back_row()]).allowed_mentions(CreateAllowedMentions::new())
}

/// Acknowledge first (the work may take over 3 s), run the save, show the result.
async fn finish(ctx: &serenity::Context, ci: &ComponentInteraction, what: ConfigId, work: impl Future<Output = Result<CreateEmbed, Error>>) -> Result<(), Error> {
    ci.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
    let embed = outcome_of(what, false, work.await);
    ci.edit_response(&ctx.http, edit_with(embed)).await?;
    Ok(())
}

pub async fn on_component(ctx: &serenity::Context, data: &Data, ci: &ComponentInteraction, id: ConfigId) -> Result<(), Error> {
    let inv = Invoker::from_interaction(ctx, data, ci.guild_id.expect("checked in handle"), &ci.user);
    let in_ephemeral = ci.message.flags.is_some_and(|f| f.contains(MessageFlags::EPHEMERAL));
    let single = |ci: &ComponentInteraction| match &ci.data.kind {
        ComponentInteractionDataKind::StringSelect { values } => values.first().cloned(),
        _ => None,
    };
    match id {
        ConfigId::Open(p) => {
            let v = page(&inv, p).await.unwrap_or_else(|e| (outcome_of(id, false, Err(e)), vec![view::config_back_row()]));
            ci.create_response(&ctx.http, show_views(in_ephemeral, v)).await?;
        }
        ConfigId::Roles(which) => {
            let ComponentInteractionDataKind::RoleSelect { values } = &ci.data.kind else { return stale(ctx, ci).await };
            finish(ctx, ci, id, core::set_role_list(&inv, which, values.clone())).await?;
        }
        ConfigId::LogCat => {
            let Some(cat) = single(ci).and_then(|v| LogCategory::ALL.into_iter().find(|c| c.as_str() == v)) else { return stale(ctx, ci).await };
            ci.create_response(&ctx.http, show_views(in_ephemeral, log_prompt(cat))).await?;
        }
        ConfigId::LogChan(cat) => {
            let ComponentInteractionDataKind::ChannelSelect { values } = &ci.data.kind else { return stale(ctx, ci).await };
            let Some(&ch) = values.first() else { return stale(ctx, ci).await };
            finish(ctx, ci, id, core::set_log_channel(&inv, cat, Some(ch))).await?;
        }
        ConfigId::LogOff(cat) => finish(ctx, ci, id, core::set_log_channel(&inv, cat, None)).await?,
        ConfigId::DmKind => {
            let Some(kind) = single(ci).and_then(|v| CaseKind::from_str(&v).ok()) else { return stale(ctx, ci).await };
            open_modal(ctx, ci, id, dm_kind_modal(&inv, kind).await, in_ephemeral).await?;
        }
        ConfigId::DmShowMod(v) => finish(ctx, ci, id, core::set_dm_showmod(&inv, v)).await?,
        ConfigId::Form(f) => open_modal(ctx, ci, id, form_modal(&inv, f).await, in_ephemeral).await?,
        ConfigId::DecayOff => finish(ctx, ci, id, core::set_decay(&inv, None)).await?,
        ConfigId::FilterSet(on) => finish(ctx, ci, id, core::set_nick_filter(&inv, on)).await?,
        ConfigId::DmText(_) | ConfigId::Submit(_) => return stale(ctx, ci).await,
    }
    Ok(())
}

/// Show the pop-up, or the refusal when the Manage Server check failed.
async fn open_modal(ctx: &serenity::Context, ci: &ComponentInteraction, id: ConfigId, modal: Result<CreateModal, Error>, in_ephemeral: bool) -> Result<(), Error> {
    let reply = match modal {
        Ok(m) => CreateInteractionResponse::Modal(m),
        Err(e) => show_views(in_ephemeral, (outcome_of(id, false, Err(e)), vec![view::config_back_row()])),
    };
    ci.create_response(&ctx.http, reply).await?;
    Ok(())
}

async fn submit(inv: &Invoker<'_>, id: ConfigId, v: &[String]) -> Result<CreateEmbed, Error> {
    let val = |i: usize| v.get(i).map_or("", |s| s.trim());
    match id {
        ConfigId::DmText(kind) => core::set_dm_template(inv, kind, Some(val(0).to_string()).filter(|t| !t.is_empty())).await,
        ConfigId::Submit(Form::LadderAdd) => {
            let duration_secs = if val(2).is_empty() { None } else { Some(parse_duration(val(2))?.num_seconds()) };
            let rule = LadderRule { warn_count: parse_whole(val(0), "Warnings")?, action: parse_ladder_action(val(1))?, duration_secs };
            core::ladder_add(inv, rule).await
        }
        ConfigId::Submit(Form::LadderRemove) => core::ladder_remove(inv, parse_whole(val(0), "Warnings")?).await,
        ConfigId::Submit(Form::DecayOn) => core::set_decay(inv, Some(parse_whole(val(0), "Days")?)).await,
        ConfigId::Submit(Form::Words) => core::set_nick_words(inv, parse_words(val(0))).await,
        ConfigId::Submit(Form::Hoist) => core::set_hoist(inv, val(0).to_string()).await,
        ConfigId::Submit(Form::Fallback) => core::set_fallback(inv, val(0).to_string()).await,
        _ => Err(bad(STALE).into()),
    }
}

pub async fn on_modal(ctx: &serenity::Context, data: &Data, mi: &ModalInteraction, id: ConfigId) -> Result<(), Error> {
    if !matches!(id, ConfigId::DmText(_) | ConfigId::Submit(_)) {
        mi.create_response(&ctx.http, CreateInteractionResponse::Message(msg(refused(STALE, false), vec![]).ephemeral(true))).await?;
        return Ok(());
    }
    mi.create_response(&ctx.http, CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true))).await?;
    let inv = Invoker::from_interaction(ctx, data, mi.guild_id.expect("checked in handle"), &mi.user);
    let embed = outcome_of(id, false, submit(&inv, id, &modal_values(mi)).await);
    mi.edit_response(&ctx.http, edit_with(embed)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_trimmed_lowercased_and_deduped() {
        assert_eq!(parse_words(" Bad, ,WORD ,x "), ["bad", "word", "x"]);
        assert_eq!(parse_words("a,A, a"), ["a"]);
        assert!(parse_words(" , ,").is_empty());
    }

    #[test]
    fn ladder_inputs() {
        assert_eq!(parse_ladder_action(" Kick ").unwrap(), LadderAction::Kick);
        assert!(parse_ladder_action("mute").is_err());
        assert_eq!(parse_whole(" 3 ", "Warnings").unwrap(), 3);
        assert!(parse_whole("x", "Warnings").is_err());
    }

    #[test]
    fn modals_fit_discord() {
        let s = GuildSettings::defaults(1);
        for f in [Form::LadderAdd, Form::LadderRemove, Form::DecayOn, Form::Words, Form::Hoist, Form::Fallback] {
            let (title, rows) = form_inputs(f, &s);
            assert!(title.chars().count() <= 45 && (1..=5).contains(&rows.len()));
            let v = serde_json::to_value(CreateModal::new(PanelId::Config(ConfigId::Submit(f)).encode(), title).components(rows)).unwrap();
            assert!(v["custom_id"].as_str().unwrap().len() <= 100);
            for r in v["components"].as_array().unwrap() {
                assert!(r["components"][0]["label"].as_str().unwrap().chars().count() <= 45);
            }
        }
        for k in templates::dm_kinds() {
            let v = serde_json::to_value(dm_modal(*k, &"x".repeat(2000))).unwrap();
            assert!(v["title"].as_str().unwrap().chars().count() <= 45);
            assert_eq!(v["components"][0]["components"][0]["value"].as_str().unwrap().chars().count(), 1500);
        }
    }
}
