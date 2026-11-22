use super::id::{Action, Category, PanelId};
use super::view;
use crate::duration::parse_duration;
use crate::invoker::Invoker;
use crate::moderation::cases::CaseKind;
use crate::moderation::commands::cases::{case_reason_core, case_remove_core, case_view_core, history_core, note_add_core, note_list_core, note_remove_core, note_view_core};
use crate::moderation::commands::nick::set_nick;
use crate::moderation::commands::punish::punish;
use crate::moderation::commands::voice::voice_action;
use crate::moderation::perms::authorize;
use crate::moderation::purge::{self, parse_amount};
use crate::reply::{refused, Tone};
use crate::safety::quarantine::{deny, handled_card, release_core};
use crate::safety::store;
use crate::safety::verify;
use crate::{Data, Error, UserError};
use poise::serenity_prelude::{
    self as serenity, ActionRowComponent, ChannelId, ChannelType, ComponentInteraction, ComponentInteractionDataKind, CreateActionRow, CreateAllowedMentions,
    CreateEmbed, CreateInputText, CreateInteractionResponse, CreateInteractionResponseFollowup, CreateInteractionResponseMessage, CreateModal, CreateSelectMenu, CreateSelectMenuKind,
    EditInteractionResponse, EditMessage, InputTextStyle, MessageFlags, ModalInteraction, Permissions, UserId,
};

pub(super) const STALE: &str = "This panel is out of date — ask an admin to run /config panel again.";
const GENERIC: &str = "Something went wrong. The error was logged.";

// ---- pure helpers ----

fn bad(msg: &str) -> UserError {
    UserError(msg.into())
}

fn parse_user_id(s: &str) -> Result<UserId, UserError> {
    match s.trim().parse::<u64>() {
        Ok(n) if n > 0 => Ok(UserId::new(n)),
        _ => Err(bad("User ID must be a number.")),
    }
}

fn parse_case_number(s: &str) -> Result<i32, UserError> {
    s.trim().parse::<i32>().ok().filter(|n| *n >= 1).ok_or_else(|| bad("Case number must be a whole number."))
}

fn parse_note_id(s: &str) -> Result<i64, UserError> {
    s.trim().parse::<i64>().ok().filter(|n| *n >= 1).ok_or_else(|| bad("Note ID must be a whole number."))
}

fn parse_delete_days(s: &str) -> Result<u8, UserError> {
    let s = s.trim();
    if s.is_empty() {
        return Ok(0);
    }
    s.parse::<u8>().ok().filter(|n| *n <= 7).ok_or_else(|| bad("Delete days must be a whole number from 0 to 7."))
}

/// Pop-up inputs for an action as (label, required), in order. Empty: no pop-up.
fn modal_inputs(a: Action) -> Vec<(&'static str, bool)> {
    const REASON: (&str, bool) = ("Reason", false);
    const DURATION: (&str, bool) = ("Duration (30m, 2h, 1d)", true);
    const DAYS: (&str, bool) = ("Delete days of messages (0-7)", false);
    match a {
        Action::Warn => vec![("Reason", true)],
        Action::Timeout => vec![DURATION, REASON],
        Action::Untimeout | Action::Kick | Action::NickReset | Action::Disconnect | Action::VoiceMute | Action::VoiceUnmute | Action::Move => vec![REASON],
        Action::Tempban => vec![DURATION, DAYS, REASON],
        Action::Ban => vec![DAYS, REASON],
        Action::Unban => vec![("User ID", true), REASON],
        Action::NickSet => vec![("Nickname", true), REASON],
        Action::Purge => vec![("How much", true), ("Only from member (user ID)", false), REASON],
        Action::History | Action::NoteList => vec![],
        Action::CaseView | Action::CaseRemove => vec![("Case number", true)],
        Action::CaseReason => vec![("Case number", true), ("New reason", true)],
        Action::NoteAdd => vec![("Note", true)],
        Action::NoteView | Action::NoteDelete => vec![("Note ID", true)],
    }
}

/// Actions that start with a member select.
fn needs_member(a: Action) -> bool {
    !matches!(a, Action::Unban | Action::Purge | Action::CaseView | Action::CaseReason | Action::CaseRemove | Action::NoteView | Action::NoteDelete)
}

// ---- running an action ----

/// Run the core for `a`. `v` holds the pop-up values in input order (empty when the action has no pop-up).
async fn exec(inv: &Invoker<'_>, a: Action, user: Option<UserId>, chan: Option<ChannelId>, v: &[String]) -> Result<CreateEmbed, Error> {
    let val = |i: usize| v.get(i).map_or("", |s| s.trim());
    let reason = |i: usize| Some(val(i).to_string()).filter(|s| !s.is_empty());
    let member = || user.ok_or_else(|| bad("Pick a member first."));
    let voice = |kind: CaseKind| async move {
        if kind == CaseKind::VoiceMove && chan.is_none() {
            return Err(bad("Choose a voice channel to move them to.").into());
        }
        voice_action(inv, member()?, kind, chan, reason(0)).await
    };
    match a {
        Action::Warn => punish(inv, CaseKind::Warn, member()?, reason(0), None, 0).await,
        Action::Untimeout => punish(inv, CaseKind::Untimeout, member()?, reason(0), None, 0).await,
        Action::Kick => punish(inv, CaseKind::Kick, member()?, reason(0), None, 0).await,
        Action::Timeout => punish(inv, CaseKind::Timeout, member()?, reason(1), Some(parse_duration(val(0))?), 0).await,
        Action::Tempban => {
            let (d, days) = (parse_duration(val(0))?, parse_delete_days(val(1))?);
            punish(inv, CaseKind::Tempban, member()?, reason(2), Some(d), days).await
        }
        Action::Ban => punish(inv, CaseKind::Ban, member()?, reason(1), None, parse_delete_days(val(0))?).await,
        Action::Unban => punish(inv, CaseKind::Unban, parse_user_id(val(0))?, reason(1), None, 0).await,
        Action::NickSet => {
            if val(0).is_empty() {
                return Err(bad("Enter a nickname.").into());
            }
            set_nick(inv, member()?, Some(val(0).to_string()), reason(1)).await
        }
        Action::NickReset => set_nick(inv, member()?, None, reason(0)).await,
        Action::Disconnect => voice(CaseKind::VoiceDisconnect).await,
        Action::VoiceMute => voice(CaseKind::VoiceMute).await,
        Action::VoiceUnmute => voice(CaseKind::VoiceUnmute).await,
        Action::Move => voice(CaseKind::VoiceMove).await,
        Action::Purge => {
            let amount = parse_amount(val(0))?;
            let only = if val(1).is_empty() { None } else { Some(parse_user_id(val(1))?) };
            let channel = chan.ok_or_else(|| bad("Pick a channel first."))?;
            purge::run(inv, channel, amount, only, reason(2)).await
        }
        Action::History => history_core(inv, member()?).await,
        Action::NoteList => note_list_core(inv, member()?).await,
        Action::NoteAdd => note_add_core(inv, member()?, val(0).to_string()).await,
        Action::CaseView => case_view_core(inv, parse_case_number(val(0))?).await,
        Action::CaseReason => case_reason_core(inv, parse_case_number(val(0))?, val(1).to_string()).await,
        Action::CaseRemove => case_remove_core(inv, parse_case_number(val(0))?).await,
        Action::NoteView => note_view_core(inv, parse_note_id(val(0))?).await,
        Action::NoteDelete => note_remove_core(inv, parse_note_id(val(0))?).await,
    }
}

/// Same wording as the slash-command error handler: refusals are shown, anything else is logged.
pub(crate) fn outcome_of(what: impl std::fmt::Debug, dm: bool, res: Result<CreateEmbed, Error>) -> CreateEmbed {
    match res {
        Ok(e) => e,
        Err(e) => match e.downcast_ref::<UserError>() {
            Some(u) => refused(&u.0, dm),
            None => {
                tracing::error!("panel action {what:?} failed: {e:?}");
                refused(GENERIC, false)
            }
        },
    }
}

fn outcome(a: Action, res: Result<CreateEmbed, Error>) -> CreateEmbed {
    outcome_of(a, false, res)
}

// ---- response builders ----

pub(crate) fn msg(embed: CreateEmbed, rows: Vec<CreateActionRow>) -> CreateInteractionResponseMessage {
    CreateInteractionResponseMessage::new().embed(embed).components(rows).allowed_mentions(CreateAllowedMentions::new())
}

fn result_edit(embed: CreateEmbed) -> EditInteractionResponse {
    EditInteractionResponse::new().embed(embed).components(vec![view::back_row()]).allowed_mentions(CreateAllowedMentions::new())
}

fn prompt(a: Action, what: &str, select: CreateSelectMenu) -> CreateInteractionResponse {
    let e = CreateEmbed::new().colour(Tone::Panel.colour()).title(format!("{} — {what}", view::action_label(a)));
    CreateInteractionResponse::UpdateMessage(msg(e, vec![CreateActionRow::SelectMenu(select), view::back_row()]))
}

fn channel_menu(id: PanelId, types: Vec<ChannelType>, placeholder: &str) -> CreateSelectMenu {
    CreateSelectMenu::new(id.encode(), CreateSelectMenuKind::Channel { channel_types: Some(types), default_channels: None }).placeholder(placeholder)
}

fn modal_for(a: Action, user: Option<UserId>, chan: Option<ChannelId>) -> Option<CreateModal> {
    let inputs = modal_inputs(a);
    if inputs.is_empty() {
        return None;
    }
    let label = view::action_label(a);
    let title: String = label.split_once(' ').map_or(label, |(_, t)| t).chars().take(45).collect();
    let rows = inputs
        .into_iter()
        .enumerate()
        .map(|(i, (label, required))| {
            let long = matches!(label, "Reason" | "New reason" | "Note");
            let style = if long { InputTextStyle::Paragraph } else { InputTextStyle::Short };
            let mut t = CreateInputText::new(style, label, i.to_string()).required(required);
            if long {
                t = t.max_length(1000);
            } else if label == "Nickname" {
                t = t.max_length(32);
            } else if label == "How much" {
                t = t.placeholder("50  ·  or  ·  10m, 2h");
            }
            CreateActionRow::InputText(t)
        })
        .collect();
    Some(CreateModal::new(PanelId::Modal(a, user, chan).encode(), title).components(rows))
}

/// Ids any home-guild member may press (no staff check); every other `mp:` id is staff-gated by its core.
pub fn is_public_id(id: &str) -> bool {
    matches!(id, "mp:v" | "mp:vq")
}

// ---- entry point ----

/// Handle a panel button, select or pop-up. Ignores everything else; never returns an error.
pub async fn handle(ctx: &serenity::Context, data: &Data, interaction: &serenity::Interaction) {
    let res = match interaction {
        serenity::Interaction::Component(ci) if ci.data.custom_id.starts_with("mp:") && ci.guild_id == Some(data.home_guild) => on_component(ctx, data, ci).await,
        serenity::Interaction::Modal(mi) if mi.data.custom_id.starts_with("mp:") && mi.guild_id == Some(data.home_guild) => on_modal(ctx, data, mi).await,
        _ => return,
    };
    if let Err(e) = res {
        tracing::error!("panel interaction failed: {e:?}");
    }
}

async fn on_component(ctx: &serenity::Context, data: &Data, ci: &ComponentInteraction) -> Result<(), Error> {
    let Some(id) = PanelId::decode(&ci.data.custom_id) else { return stale(ctx, ci).await };
    let in_ephemeral = ci.message.flags.is_some_and(|f| f.contains(MessageFlags::EPHEMERAL));
    // Panel navigation edits an ephemeral reply in place; the public panel gets a new ephemeral one.
    let show = |v| show_views(in_ephemeral, v);
    let reply = match id {
        PanelId::Home => show(view::home()),
        PanelId::Category(Category::Config) => {
            let inv = Invoker::from_interaction(ctx, data, ci.guild_id.expect("checked in handle"), &ci.user);
            show(super::config::category(&inv).await)
        }
        PanelId::Category(c) => show(view::category(c)),
        PanelId::Config(c) => return super::config::on_component(ctx, data, ci, c).await,
        PanelId::Review(approve, user) => return on_review(ctx, data, ci, approve, user).await,
        PanelId::Verify => return verify::on_press(ctx, data, ci).await,
        PanelId::Action(Action::Purge) => prompt(
            Action::Purge,
            "which channel?",
            channel_menu(PanelId::PickChannel(Action::Purge, None), vec![ChannelType::Text, ChannelType::News], "Choose a channel"),
        ),
        PanelId::Action(a) if needs_member(a) => {
            let menu = CreateSelectMenu::new(PanelId::PickUser(a).encode(), CreateSelectMenuKind::User { default_users: None }).placeholder("Choose a member");
            prompt(a, "who?", menu)
        }
        PanelId::Action(a) => return open_modal(ctx, ci, a, None, None).await,
        PanelId::PickUser(a) => {
            let ComponentInteractionDataKind::UserSelect { values } = &ci.data.kind else { return stale(ctx, ci).await };
            let Some(&user) = values.first() else { return stale(ctx, ci).await };
            match a {
                Action::Move => prompt(
                    a,
                    "to which channel?",
                    channel_menu(PanelId::PickChannel(Action::Move, Some(user)), vec![ChannelType::Voice, ChannelType::Stage], "Choose a voice channel"),
                ),
                Action::History | Action::NoteList => {
                    ci.create_response(&ctx.http, CreateInteractionResponse::Acknowledge).await?;
                    let inv = Invoker::from_interaction(ctx, data, ci.guild_id.expect("checked in handle"), &ci.user);
                    let embed = outcome(a, exec(&inv, a, Some(user), None, &[]).await);
                    ci.edit_response(&ctx.http, result_edit(embed)).await?;
                    return Ok(());
                }
                _ => return open_modal(ctx, ci, a, Some(user), None).await,
            }
        }
        PanelId::PickChannel(a, user) => {
            let ComponentInteractionDataKind::ChannelSelect { values } = &ci.data.kind else { return stale(ctx, ci).await };
            let Some(&chan) = values.first() else { return stale(ctx, ci).await };
            return open_modal(ctx, ci, a, user, Some(chan)).await;
        }
        PanelId::Modal(..) | PanelId::VerifyQuiz => return stale(ctx, ci).await,
    };
    ci.create_response(&ctx.http, reply).await?;
    Ok(())
}

/// Approve / Deny on a quarantine review card. The card is a public message, so the reply is a deferred ephemeral followup.
async fn on_review(ctx: &serenity::Context, data: &Data, ci: &ComponentInteraction, approve: bool, user: UserId) -> Result<(), Error> {
    ci.create_response(&ctx.http, CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true))).await?;
    let inv = Invoker::from_interaction(ctx, data, ci.guild_id.expect("checked in handle"), &ci.user);
    let strip = |embed: Option<CreateEmbed>| {
        let mut e = EditMessage::new().components(vec![]);
        if let Some(embed) = embed {
            e = e.embed(embed);
        }
        e
    };
    let res = async {
        authorize(&inv, Permissions::MODERATE_MEMBERS, Some(user)).await?;
        let Some(row) = store::quarantine_get(&data.db, inv.guild_id.get() as i64, user.get() as i64).await? else {
            let title = ci.message.embeds.first().and_then(|e| e.title.clone()).unwrap_or_default();
            let card = handled_card(&title, "Already handled", "Already handled");
            if let Err(e) = ci.channel_id.edit_message(ctx, ci.message.id, strip(Some(card))).await {
                tracing::warn!("marking stale review card failed: {e}");
            }
            return Err(UserError("Already handled.".into()).into());
        };
        let embed = if approve { release_core(&inv, user).await? } else { deny(&inv, user).await? };
        // Release/deny marked the stored card; a copy elsewhere (the Waiting-review message) just loses its buttons.
        if row.review_message_id != Some(ci.message.id.get() as i64) {
            if let Err(e) = ci.channel_id.edit_message(ctx, ci.message.id, strip(None)).await {
                tracing::warn!("stripping review buttons failed: {e}");
            }
        }
        Ok::<_, Error>(embed)
    }
    .await;
    let embed = outcome_of(PanelId::Review(approve, user), false, res);
    let followup = CreateInteractionResponseFollowup::new().embed(embed).ephemeral(true).allowed_mentions(CreateAllowedMentions::new());
    ci.create_followup(&ctx.http, followup).await?;
    Ok(())
}

pub(super) async fn stale(ctx: &serenity::Context, ci: &ComponentInteraction) -> Result<(), Error> {
    ci.create_response(&ctx.http, CreateInteractionResponse::Message(msg(refused(STALE, false), vec![]).ephemeral(true))).await?;
    Ok(())
}

async fn open_modal(ctx: &serenity::Context, ci: &ComponentInteraction, a: Action, user: Option<UserId>, chan: Option<ChannelId>) -> Result<(), Error> {
    match modal_for(a, user, chan) {
        Some(m) => ci.create_response(&ctx.http, CreateInteractionResponse::Modal(m)).await?,
        None => return stale(ctx, ci).await,
    }
    Ok(())
}

/// Edit a navigation message in place when it is already ephemeral, else open a new ephemeral one.
pub(super) fn show_views(in_ephemeral: bool, (e, rows): (CreateEmbed, Vec<CreateActionRow>)) -> CreateInteractionResponse {
    if in_ephemeral {
        CreateInteractionResponse::UpdateMessage(msg(e, rows))
    } else {
        CreateInteractionResponse::Message(msg(e, rows).ephemeral(true))
    }
}

/// Text input values of a submitted pop-up, in order.
pub(crate) fn modal_values(mi: &ModalInteraction) -> Vec<String> {
    mi.data
        .components
        .iter()
        .flat_map(|r| r.components.iter())
        .filter_map(|c| match c {
            ActionRowComponent::InputText(t) => Some(t.value.clone().unwrap_or_default()),
            _ => None,
        })
        .collect()
}

async fn on_modal(ctx: &serenity::Context, data: &Data, mi: &ModalInteraction) -> Result<(), Error> {
    let decoded = PanelId::decode(&mi.data.custom_id);
    if let Some(PanelId::Config(c)) = decoded {
        return super::config::on_modal(ctx, data, mi, c).await;
    }
    if decoded == Some(PanelId::VerifyQuiz) {
        return verify::on_submit(ctx, data, mi).await;
    }
    let Some(PanelId::Modal(a, user, chan)) = decoded else {
        mi.create_response(&ctx.http, CreateInteractionResponse::Message(msg(refused(STALE, false), vec![]).ephemeral(true))).await?;
        return Ok(());
    };
    mi.create_response(&ctx.http, CreateInteractionResponse::Defer(CreateInteractionResponseMessage::new().ephemeral(true))).await?;
    let values = modal_values(mi);
    let inv = Invoker::from_interaction(ctx, data, mi.guild_id.expect("checked in handle"), &mi.user);
    let embed = outcome(a, exec(&inv, a, user, chan, &values).await);
    mi.edit_response(&ctx.http, result_edit(embed)).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Action; 22] = [
        Action::Warn, Action::Timeout, Action::Untimeout, Action::Kick, Action::Tempban, Action::Ban, Action::Unban, Action::NickSet, Action::NickReset,
        Action::Disconnect, Action::VoiceMute, Action::VoiceUnmute, Action::Move, Action::Purge, Action::History, Action::CaseView, Action::CaseReason,
        Action::CaseRemove, Action::NoteAdd, Action::NoteList, Action::NoteView, Action::NoteDelete,
    ];

    #[test]
    fn public_ids() {
        assert!(is_public_id("mp:v") && is_public_id("mp:vq"));
        for id in ["mp:home", "mp:h", "mp:rv:a:1", "mp:vx", "v"] {
            assert!(!is_public_id(id), "{id}");
        }
    }

    #[test]
    fn parsers() {
        assert_eq!(parse_user_id(" 123 ").unwrap().get(), 123);
        for bad in ["", "x", "0", "-5", "1 2"] {
            assert!(parse_user_id(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_case_number("12").unwrap(), 12);
        for bad in ["", "0", "-1", "x", "99999999999"] {
            assert!(parse_case_number(bad).is_err(), "{bad}");
        }
        assert_eq!(parse_delete_days("").unwrap(), 0);
        assert_eq!(parse_delete_days("7").unwrap(), 7);
        for bad in ["8", "x", "-1"] {
            assert!(parse_delete_days(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn modal_inputs_fit_discord() {
        for a in ALL {
            let i = modal_inputs(a);
            assert!(i.len() <= 5, "{a:?}");
            assert!(i.iter().all(|(l, _)| l.chars().count() <= 45));
            assert_eq!(modal_for(a, None, None).is_some(), !i.is_empty(), "{a:?}");
            // Every action is reachable: a pop-up, or a member select that runs it directly.
            assert!(!i.is_empty() || needs_member(a), "{a:?}");
        }
        assert!(modal_inputs(Action::History).is_empty());
        assert_eq!(modal_inputs(Action::Tempban), vec![("Duration (30m, 2h, 1d)", true), ("Delete days of messages (0-7)", false), ("Reason", false)]);
    }

    #[test]
    fn modal_titles_and_ids_fit() {
        for a in ALL.into_iter().filter(|a| !modal_inputs(*a).is_empty()) {
            let v = serde_json::to_value(modal_for(a, Some(UserId::new(u64::MAX)), Some(ChannelId::new(u64::MAX))).unwrap()).unwrap();
            assert!(v["title"].as_str().unwrap().chars().count() <= 45);
            assert!(v["custom_id"].as_str().unwrap().len() <= 100);
        }
    }
}
