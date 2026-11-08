//! The screener: everyone with mail in the screened folder, one row per sender, and a key per
//! decision. A decision puts the sender in a Fastmail contact group (which a rule the user made
//! once routes on) and moves what they already sent. The setup page makes the folder and groups
//! and walks through the rules Fastmail only lets people make by hand.

use std::collections::BTreeSet;

use rune::{Key, KeyEvent, Tx};
use schlau_store::{Action, ContactGroup, ScreenerSender};
use schlau_sync::{CONTACTS_AVAILABLE, Command, Destination, DestinationKind, ScreenerConfig, SetupRequest, group_name};

use crate::state::{PalettePurpose, Ui};

/// Where Fastmail's rules are edited.
pub const FASTMAIL_RULES_URL: &str = "https://app.fastmail.com/settings/filters";
/// The folder the setup makes when the account has none to screen in.
pub const TRIAGE_NAME: &str = "Triage";

pub struct Screener {
    pub senders: Vec<ScreenerSender>,
    pub selected: usize,
    /// Senders marked with `x`, by email: the next decision applies to all of them.
    pub marked: BTreeSet<String>,
}

pub struct ScreenerSetup {
    pub picked: usize,
    /// The setup asked the engine to make the screened folder; adopt it once it syncs.
    pub awaiting_triage: bool,
}

/// A row of the setup page, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupRow {
    Contacts,
    Triage,
    Destination(usize),
    AddDestination,
    /// The rule for the destination at this index.
    Rule(usize),
    CatchAll,
}

pub fn setup_rows(config: &ScreenerConfig) -> Vec<SetupRow> {
    let destinations = (0..config.destinations.len()).map(SetupRow::Destination);
    let rules = config
        .destinations
        .iter()
        .enumerate()
        .filter(|(_, d)| d.kind != DestinationKind::Inbox)
        .map(|(i, _)| SetupRow::Rule(i));
    [SetupRow::Contacts, SetupRow::Triage]
        .into_iter()
        .chain(destinations)
        .chain([SetupRow::AddDestination])
        .chain(rules)
        .chain([SetupRow::CatchAll])
        .collect()
}

/// Whether the session offers contacts: `None` before the first sync has looked.
pub fn contacts_available(s: &Ui) -> Option<bool> {
    s.reader.meta(CONTACTS_AVAILABLE).ok().flatten().map(|v| v == "1")
}

pub fn config(s: &Ui) -> ScreenerConfig {
    s.settings.effective().screener
}

pub fn mailbox_name(s: &Ui, id: &str) -> String {
    s.mailboxes.iter().find(|m| m.id == id).map(|m| m.name.clone()).unwrap_or_else(|| "a missing folder".into())
}

/// The file destinations in order: `1` files in the first, `2` in the second.
pub fn file_destinations(config: &ScreenerConfig) -> Vec<usize> {
    config.destinations.iter().enumerate().filter(|(_, d)| d.kind == DestinationKind::File).map(|(i, _)| i).take(9).collect()
}

pub fn destination_of(config: &ScreenerConfig, kind: DestinationKind) -> Option<usize> {
    config.destinations.iter().position(|d| d.kind == kind)
}

/// The digit or key that sends a sender to destination `index`.
pub fn destination_key(config: &ScreenerConfig, index: usize) -> String {
    match config.destinations.get(index).map(|d| d.kind) {
        Some(DestinationKind::Inbox) => "y".into(),
        Some(DestinationKind::Out) => "!".into(),
        _ => file_destinations(config).iter().position(|&i| i == index).map(|n| (n + 1).to_string()).unwrap_or_default(),
    }
}

fn group_id(groups: &[ContactGroup], name: &str) -> Option<String> {
    groups.iter().find(|g| g.name == name).map(|g| g.id.clone())
}

/// Opens the screener, or its setup when there is nothing to screen with yet.
pub fn open(s: &mut Ui) {
    if !config(s).is_active() {
        return open_setup(s);
    }
    s.close_thread();
    s.settings_pane = None;
    s.screener_setup = None;
    let senders = load(s);
    s.screener = Some(Screener { senders, selected: 0, marked: BTreeSet::new() });
}

pub fn open_setup(s: &mut Ui) {
    s.close_thread();
    s.settings_pane = None;
    s.screener = None;
    s.screener_setup = Some(ScreenerSetup { picked: 0, awaiting_triage: false });
    ensure_groups(s);
}

pub fn close(s: &mut Ui) {
    s.screener = None;
    s.screener_setup = None;
}

/// The senders in the screened folder: those with a suggestion first, each part newest first.
fn load(s: &Ui) -> Vec<ScreenerSender> {
    let config = config(s);
    let groups = s.reader.contact_groups().unwrap_or_default();
    let mut senders = config.triage.as_ref().and_then(|triage| s.reader.screener_senders(triage).ok()).unwrap_or_default();
    senders.sort_by_key(|x| suggestion(&config, &groups, x).is_none());
    senders
}

/// Re-reads the senders after the replica changed, keeping the selection on the same sender, and
/// adopts the screened folder the setup asked for once it has synced.
pub fn reload(s: &mut Ui) {
    if s.screener.is_some() {
        let senders = load(s);
        if let Some(screener) = s.screener.as_mut() {
            let keep = screener.senders.get(screener.selected).map(|x| x.email.clone());
            screener.marked.retain(|email| senders.iter().any(|x| &x.email == email));
            screener.selected = keep
                .and_then(|email| senders.iter().position(|x| x.email == email))
                .unwrap_or(screener.selected)
                .min(senders.len().saturating_sub(1));
            screener.senders = senders;
        }
    }
    let awaiting = s.screener_setup.as_ref().is_some_and(|x| x.awaiting_triage) && config(s).triage.is_none();
    if awaiting && let Some(id) = existing_triage(s) {
        set_triage(s, &id);
    }
}

fn existing_triage(s: &Ui) -> Option<String> {
    s.mailboxes.iter().find(|m| m.parent_id.is_none() && m.name.eq_ignore_ascii_case(TRIAGE_NAME)).map(|m| m.id.clone())
}

/// Saves the setup, tells the engine, and asks it for any group that doesn't exist yet.
fn save(s: &mut Ui, config: ScreenerConfig) {
    if let Err(e) = s.settings.set_screener(config.clone()) {
        s.toast(format!("Screener not saved: {e}"));
    }
    if let Some(engine) = &s.engine {
        engine.send(Command::Screener(config));
    }
    ensure_groups(s);
}

/// Asks the engine to make the destinations' groups that the replica doesn't have.
fn ensure_groups(s: &mut Ui) {
    if contacts_available(s) != Some(true) {
        return;
    }
    let known = s.reader.contact_groups().unwrap_or_default();
    let missing: Vec<String> =
        config(s).destinations.iter().map(|d| d.group.clone()).filter(|g| group_id(&known, g).is_none()).collect();
    if let (false, Some(engine)) = (missing.is_empty(), &s.engine) {
        engine.send(Command::SetupScreener(SetupRequest { triage: None, groups: missing }));
    }
}

/// Screens new senders in `id`, and adds the Inbox and spam destinations the first time.
pub fn set_triage(s: &mut Ui, id: &str) {
    let mut config = config(s);
    config.triage = Some(id.to_string());
    let defaults = [(DestinationKind::Inbox, "inbox", "Inbox"), (DestinationKind::Out, "junk", "Out")];
    defaults.into_iter().for_each(|(kind, role, name)| {
        if destination_of(&config, kind).is_none()
            && let Some(mailbox) = s.mailbox_with_role(role)
        {
            config.destinations.push(Destination { kind, mailbox_id: mailbox.id.clone(), group: group_name(name) });
        }
    });
    if let Some(setup) = s.screener_setup.as_mut() {
        setup.awaiting_triage = false;
    }
    let name = mailbox_name(s, id);
    save(s, config);
    s.toast(format!("New senders are screened in {name}"));
}

/// Adds folder `id` as a place to file screened senders in.
pub fn add_destination(s: &mut Ui, id: &str) {
    let mut config = config(s);
    if config.triage.as_deref() == Some(id) {
        return s.toast("That is the folder new senders land in");
    }
    if config.destinations.iter().any(|d| d.mailbox_id == id) {
        return s.toast("Already a destination");
    }
    if file_destinations(&config).len() >= 9 {
        return s.toast("Nine folders is the most: one per digit");
    }
    let name = mailbox_name(s, id);
    config.destinations.push(Destination { kind: DestinationKind::File, mailbox_id: id.to_string(), group: group_name(&name) });
    save(s, config);
    s.toast(format!("{name} added: now add its rule in Fastmail"));
}

/// Stops offering a file destination. Its group stays in Fastmail, with its members.
pub fn remove_destination(s: &mut Ui, index: usize) {
    let mut config = config(s);
    if config.destinations.get(index).is_none_or(|d| d.kind != DestinationKind::File) {
        return s.toast("Inbox and spam stay; only folders can be removed");
    }
    let removed = config.destinations.remove(index);
    let name = mailbox_name(s, &removed.mailbox_id);
    save(s, config);
    s.toast(format!("{name} removed. Its group and rule stay in Fastmail"));
}

/// The destination a sender probably belongs in: where other addresses at their domain were sent,
/// else the folder their earlier mail is in.
pub fn suggestion(config: &ScreenerConfig, groups: &[ContactGroup], sender: &ScreenerSender) -> Option<usize> {
    let by_group = sender.domain_groups.iter().find_map(|(group, _)| {
        config.destinations.iter().position(|d| group_id(groups, &d.group).as_deref() == Some(group.as_str()))
    });
    let by_folder = || {
        sender.elsewhere.iter().find_map(|(mailbox, _)| {
            config.destinations.iter().position(|d| d.kind == DestinationKind::File && &d.mailbox_id == mailbox)
        })
    };
    by_group.or_else(by_folder)
}

/// What sending `sender` to destination `index` does, and what undoes it.
fn decision(s: &Ui, config: &ScreenerConfig, groups: &[ContactGroup], sender: &ScreenerSender, index: usize) -> Result<(Vec<Action>, Vec<Action>), String> {
    let triage = config.triage.clone().ok_or("No folder is screened yet")?;
    let destination = config.destinations.get(index).ok_or("No such destination")?;
    let group = group_id(groups, &destination.group)
        .ok_or_else(|| format!("The group “{}” isn't in Fastmail yet: open the screener setup", destination.group))?;
    let card = s.reader.card_for_email(&sender.email).map_err(|e| e.to_string())?;
    let made = card.is_none();
    let uid = card.map(|(uid, _)| uid).unwrap_or_else(|| format!("urn:uuid:{}", schlau_sync::new_uuid()));
    let current = s.reader.groups_of(&sender.email).map_err(|e| e.to_string())?;
    let leave = config
        .destinations
        .iter()
        .filter_map(|d| group_id(groups, &d.group))
        .filter(|g| *g != group && current.contains(g))
        .map(|g| Action::RemoveFromGroup {
            group_id: g,
            email: sender.email.clone(),
            name: sender.name.clone(),
            uid: uid.clone(),
            delete_card: false,
        });
    let join = Action::AddToGroup { group_id: group.clone(), email: sender.email.clone(), name: sender.name.clone(), uid: uid.clone() };
    let route = schlau_sync::Route { group_id: group.clone(), mailbox_id: destination.mailbox_id.clone(), kind: destination.kind };
    let moved = schlau_sync::route_actions(sender.message_ids.clone(), &triage, &route);
    let seen = (destination.kind == DestinationKind::Inbox)
        .then(|| Action::SetFlag { message_ids: sender.message_ids.clone(), flag: "$seen".into(), on: true });
    let actions: Vec<Action> = leave.chain([join]).chain(moved).chain(seen).collect();
    let undo = actions
        .iter()
        .rev()
        .map(|a| match a {
            Action::AddToGroup { group_id, email, name, uid } => Action::RemoveFromGroup {
                group_id: group_id.clone(),
                email: email.clone(),
                name: name.clone(),
                uid: uid.clone(),
                delete_card: made,
            },
            other => crate::state::inverse(other),
        })
        .collect();
    Ok((actions, undo))
}

/// The senders a decision applies to: the marked ones, else the selected one.
fn targets(s: &Ui) -> Vec<ScreenerSender> {
    let Some(screener) = s.screener.as_ref() else { return Vec::new() };
    match screener.marked.is_empty() {
        true => screener.senders.get(screener.selected).cloned().into_iter().collect(),
        false => screener.senders.iter().filter(|x| screener.marked.contains(&x.email)).cloned().collect(),
    }
}

/// Sends `senders` where `choose` says, as one undoable step, and takes them off the list.
fn decide_each(s: &mut Ui, senders: Vec<(ScreenerSender, usize)>) {
    if senders.is_empty() {
        return;
    }
    let config = config(s);
    let groups = s.reader.contact_groups().unwrap_or_default();
    let planned: Result<Vec<_>, String> =
        senders.iter().map(|(sender, index)| decision(s, &config, &groups, sender, *index)).collect();
    let planned = match planned {
        Ok(p) => p,
        Err(e) => return s.toast(e),
    };
    let (actions, undo) = planned.into_iter().fold((Vec::new(), Vec::new()), |(mut actions, mut undo), (a, u)| {
        actions.extend(a);
        undo.splice(0..0, u);
        (actions, undo)
    });
    s.perform_with_undo(actions, undo);
    let decided: BTreeSet<String> = senders.iter().map(|(x, _)| x.email.clone()).collect();
    if let Some(screener) = s.screener.as_mut() {
        screener.senders.retain(|x| !decided.contains(&x.email));
        screener.marked.clear();
        screener.selected = screener.selected.min(screener.senders.len().saturating_sub(1));
    }
    let places: BTreeSet<usize> = senders.iter().map(|(_, i)| *i).collect();
    let place = match (places.len(), places.first().and_then(|&i| config.destinations.get(i))) {
        (1, Some(d)) => match d.kind {
            DestinationKind::Inbox => "Let in".to_string(),
            DestinationKind::Out => "Sent to spam".to_string(),
            DestinationKind::File => format!("Filed in {}", mailbox_name(s, &d.mailbox_id)),
        },
        _ => "Decided".to_string(),
    };
    let who = match senders.len() {
        1 => senders[0].0.name.clone().chars().take(40).collect::<String>(),
        n => format!("{n} senders"),
    };
    let who = if who.trim().is_empty() { senders[0].0.email.clone() } else { who };
    s.toast(format!("{place}: {who} · z to undo"));
}

/// Sends the marked senders, or the selected one, to destination `index`.
pub fn decide(s: &mut Ui, index: Option<usize>) {
    let Some(index) = index else { return s.toast("That destination isn't set up: open the screener setup") };
    let senders = targets(s).into_iter().map(|x| (x, index)).collect();
    decide_each(s, senders);
}

/// Sends every sender with a suggestion where it points.
pub fn accept_suggestions(s: &mut Ui) {
    let config = config(s);
    let groups = s.reader.contact_groups().unwrap_or_default();
    let senders: Vec<(ScreenerSender, usize)> = s
        .screener
        .iter()
        .flat_map(|x| x.senders.iter())
        .filter_map(|x| suggestion(&config, &groups, x).map(|i| (x.clone(), i)))
        .collect();
    match senders.is_empty() {
        true => s.toast("No suggestions"),
        false => decide_each(s, senders),
    }
}

fn move_selection(s: &mut Ui, delta: isize) {
    if let Some(screener) = s.screener.as_mut().filter(|x| !x.senders.is_empty()) {
        let last = screener.senders.len() as isize - 1;
        screener.selected = (screener.selected as isize + delta).clamp(0, last) as usize;
    }
}

fn toggle_mark(s: &mut Ui) {
    let Some(screener) = s.screener.as_mut() else { return };
    let Some(email) = screener.senders.get(screener.selected).map(|x| x.email.clone()) else { return };
    if !screener.marked.remove(&email) {
        screener.marked.insert(email);
    }
    move_selection(s, 1);
}

fn open_latest(s: &mut Ui) {
    let thread = s.screener.as_ref().and_then(|x| x.senders.get(x.selected)).map(|x| x.latest_thread.clone());
    if let Some(thread) = thread {
        s.open_thread(&thread);
    }
}

pub fn row_key(email: &str) -> String {
    format!("sender:{email}")
}

/// A key typed as text, without ⌘ or Ctrl: arrows as j/k/h/l, Return and Escape by name.
fn typed(ev: &KeyEvent) -> Option<String> {
    match &ev.key {
        Key::Text(t) if !ev.mods.cmd && !ev.mods.ctrl => Some(match (t.as_str(), ev.mods.shift) {
            ("a" | "A", true) | ("A", false) => "A".to_string(),
            _ => t.clone(),
        }),
        Key::Down => Some("j".into()),
        Key::Up => Some("k".into()),
        Key::Enter => Some("enter".into()),
        Key::Escape => Some("escape".into()),
        _ => None,
    }
}

/// Keys while the screener list is showing. What it doesn't use goes to the keymap.
pub fn on_key(s: &mut Ui, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.pending_chord.is_some() {
        return crate::commands::dispatch(s, ev, tx);
    }
    let config = config(s);
    let handled = match typed(ev).as_deref() {
        Some("j") => {
            move_selection(s, 1);
            true
        }
        Some("k") => {
            move_selection(s, -1);
            true
        }
        Some("y") => {
            decide(s, destination_of(&config, DestinationKind::Inbox));
            true
        }
        Some("!") => {
            decide(s, destination_of(&config, DestinationKind::Out));
            true
        }
        Some(d) if d.len() == 1 && d.chars().all(|c| ('1'..='9').contains(&c)) => {
            let n = d.parse::<usize>().unwrap_or(0);
            decide(s, file_destinations(&config).get(n.saturating_sub(1)).copied());
            true
        }
        Some("x") => {
            toggle_mark(s);
            true
        }
        Some("A") => {
            accept_suggestions(s);
            true
        }
        Some("enter" | "o" | "l") => {
            open_latest(s);
            true
        }
        Some("escape" | "q" | "u" | "h") => {
            close(s);
            true
        }
        _ => return crate::commands::dispatch(s, ev, tx),
    };
    if let Some(email) = s.screener.as_ref().and_then(|x| x.senders.get(x.selected)).map(|x| x.email.clone()) {
        tx.scroll_into_view(row_key(&email));
    }
    handled
}

fn open_url(s: &mut Ui, url: &str) {
    match std::process::Command::new("open").arg(url).spawn() {
        Ok(_) => s.toast("Opened Fastmail's rules in the browser"),
        Err(e) => s.toast(format!("Couldn't open {url}: {e}")),
    }
}

/// What `c` copies on a setup row: the value typed into Fastmail.
pub fn copy_value(s: &Ui, row: SetupRow) -> Option<String> {
    let config = config(s);
    match row {
        SetupRow::Rule(i) | SetupRow::Destination(i) => config.destinations.get(i).map(|d| d.group.clone()),
        SetupRow::CatchAll | SetupRow::Triage => config.triage.map(|t| mailbox_name(s, &t)),
        SetupRow::Contacts | SetupRow::AddDestination => None,
    }
}

fn activate(s: &mut Ui, row: SetupRow) {
    match row {
        SetupRow::Contacts => s.toast("Fastmail → Settings → Privacy & Security → API tokens: allow Contacts"),
        SetupRow::Triage => match (config(s).triage, existing_triage(s)) {
            (None, Some(id)) => set_triage(s, &id),
            (None, None) => {
                if let Some(engine) = &s.engine {
                    engine.send(Command::SetupScreener(SetupRequest { triage: Some(TRIAGE_NAME.into()), groups: Vec::new() }));
                }
                if let Some(setup) = s.screener_setup.as_mut() {
                    setup.awaiting_triage = true;
                }
                s.toast(format!("Making the {TRIAGE_NAME} folder…"));
            }
            (Some(_), _) => crate::commands::open_palette_for(s, PalettePurpose::Triage),
        },
        SetupRow::Destination(_) => s.toast("d removes a folder; Inbox and spam stay"),
        SetupRow::AddDestination => crate::commands::open_palette_for(s, PalettePurpose::Destination),
        SetupRow::Rule(_) | SetupRow::CatchAll => open_url(s, FASTMAIL_RULES_URL),
    }
}

/// Keys while the setup page is showing. What it doesn't use goes to the keymap.
pub fn on_setup_key(s: &mut Ui, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.pending_chord.is_some() {
        return crate::commands::dispatch(s, ev, tx);
    }
    let rows = setup_rows(&config(s));
    let Some(picked) = s.screener_setup.as_ref().map(|x| x.picked.min(rows.len() - 1)) else { return false };
    let row = rows[picked];
    let step = |s: &mut Ui, picked: usize| {
        if let Some(setup) = s.screener_setup.as_mut() {
            setup.picked = picked;
        }
    };
    match typed(ev).as_deref() {
        Some("j") => step(s, (picked + 1).min(rows.len() - 1)),
        Some("k") => step(s, picked.saturating_sub(1)),
        Some("enter" | "l") => activate(s, row),
        Some("o") => open_url(s, FASTMAIL_RULES_URL),
        Some("c") => match copy_value(s, row) {
            Some(value) => {
                s.toast(format!("Copied “{value}”"));
                tx.copy(value);
            }
            None => s.toast("Nothing to copy on this row"),
        },
        Some("d" | "x") => match row {
            SetupRow::Destination(i) => remove_destination(s, i),
            _ => s.toast("Only folders can be removed"),
        },
        Some("escape" | "q" | "u" | "h") => close(s),
        _ => return crate::commands::dispatch(s, ev, tx),
    }
    if let Some(setup) = s.screener_setup.as_ref() {
        tx.scroll_into_view(crate::view::setup_row_key(setup.picked));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination(kind: DestinationKind, mailbox: &str) -> Destination {
        Destination { kind, mailbox_id: mailbox.into(), group: group_name(mailbox) }
    }

    fn sample() -> ScreenerConfig {
        ScreenerConfig {
            triage: Some("triage".into()),
            destinations: vec![
                destination(DestinationKind::Inbox, "Inbox"),
                destination(DestinationKind::Out, "Junk"),
                destination(DestinationKind::File, "News"),
                destination(DestinationKind::File, "Receipts"),
            ],
        }
    }

    #[test]
    fn digits_number_the_folders_and_letters_the_rest() {
        let config = sample();
        assert_eq!(file_destinations(&config), [2, 3]);
        assert_eq!((0..4).map(|i| destination_key(&config, i)).collect::<Vec<_>>(), ["y", "!", "1", "2"]);
    }

    #[test]
    fn setup_lists_a_rule_for_every_destination_but_the_inbox() {
        let rows = setup_rows(&sample());
        assert_eq!(rows.iter().filter(|r| matches!(r, SetupRow::Rule(_))).count(), 3);
        assert!(!rows.contains(&SetupRow::Rule(0)), "contacts already skip the catch-all");
        assert_eq!(rows.first(), Some(&SetupRow::Contacts));
        assert_eq!(rows.last(), Some(&SetupRow::CatchAll));
    }

    #[test]
    fn suggestions_follow_the_domain_then_earlier_mail() {
        let config = sample();
        let groups = vec![ContactGroup { id: "g-r".into(), name: group_name("Receipts"), members: 3 }];
        let sender = |domain: Vec<(String, i64)>, elsewhere: Vec<(String, i64)>| ScreenerSender {
            domain_groups: domain,
            elsewhere,
            ..Default::default()
        };
        assert_eq!(suggestion(&config, &groups, &sender(vec![("g-r".into(), 2)], vec![("News".into(), 5)])), Some(3));
        assert_eq!(suggestion(&config, &groups, &sender(vec![], vec![("Archive".into(), 9), ("News".into(), 1)])), Some(2));
        assert_eq!(suggestion(&config, &groups, &sender(vec![("g-unknown".into(), 1)], vec![])), None);
    }
}
