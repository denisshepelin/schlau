//! Every command the app has, in one table: the keymap and the command palette both read it, so
//! each action has one name, one shortcut and one implementation.

use std::collections::BTreeMap;

use rune::{Key, KeyEvent, TextEdit, Tx};
use schlau_store::Action;
use schlau_sync::Command as EngineCommand;

use crate::state::{Palette, Search, Ui};

pub struct Command {
    pub id: &'static str,
    pub title: &'static str,
    /// Key sequences as typed: "e", "shift+i", "g i", "mod+k". Shown in the palette.
    pub keys: &'static [&'static str],
    pub run: fn(&mut Ui, &mut Tx),
}

pub const COMMANDS: &[Command] = &[
    Command { id: "palette", title: "Command palette", keys: &["mod+k"], run: open_palette },
    Command { id: "search", title: "Search", keys: &["/"], run: open_search },
    Command { id: "next", title: "Next conversation", keys: &["j"], run: |s, _| next(s, 1) },
    Command { id: "previous", title: "Previous conversation", keys: &["k"], run: |s, _| next(s, -1) },
    Command { id: "open", title: "Open conversation", keys: &["enter", "o"], run: |s, _| s.open_selected() },
    Command { id: "back", title: "Back", keys: &["escape", "u"], run: back },
    Command { id: "archive", title: "Mark done (archive)", keys: &["e"], run: archive },
    Command { id: "trash", title: "Move to trash", keys: &["#"], run: trash },
    Command { id: "spam", title: "Mark as spam", keys: &["!"], run: spam },
    Command { id: "star", title: "Star / unstar", keys: &["s"], run: toggle_star },
    Command { id: "unread", title: "Mark unread", keys: &["shift+u"], run: |s, _| set_seen(s, false) },
    Command { id: "read", title: "Mark read", keys: &["shift+i"], run: |s, _| set_seen(s, true) },
    Command { id: "undo", title: "Undo", keys: &["z", "mod+z"], run: undo },
    Command { id: "web", title: "View original (secure web view)", keys: &["v"], run: toggle_web },
    Command { id: "expand", title: "Expand all messages", keys: &["shift+e"], run: expand_all },
    Command { id: "refresh", title: "Sync now", keys: &["mod+r"], run: refresh },
    Command { id: "semantic", title: "Enable meaning search (downloads a 530 MB model once)", keys: &[], run: enable_semantic },
    Command { id: "media", title: "Enable image and attachment search (downloads a 1.5 GB model once)", keys: &[], run: enable_media },
    Command { id: "top", title: "Jump to newest", keys: &["shift+k"], run: |s, _| next(s, isize::MIN / 2) },
    Command { id: "go-inbox", title: "Go to Inbox", keys: &["g i"], run: |s, _| go_role(s, "inbox") },
    Command { id: "go-archive", title: "Go to Archive", keys: &["g e"], run: |s, _| go_role(s, "archive") },
    Command { id: "go-sent", title: "Go to Sent", keys: &["g t"], run: |s, _| go_role(s, "sent") },
    Command { id: "go-drafts", title: "Go to Drafts", keys: &["g d"], run: |s, _| go_role(s, "drafts") },
    Command { id: "go-trash", title: "Go to Trash", keys: &["g #"], run: |s, _| go_role(s, "trash") },
    Command { id: "go-spam", title: "Go to Spam", keys: &["g !"], run: |s, _| go_role(s, "junk") },
];

/// A palette entry: a static command, or a mailbox to go to.
#[derive(Debug, Clone, PartialEq)]
pub enum Entry {
    Command(usize),
    Mailbox(String),
}

/// Finds the command bound to a key event, handling two-key chords that start with `g`.
pub fn dispatch(s: &mut Ui, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let typed = describe(ev);
    let Some(typed) = typed else { return false };
    let sequence = match s.pending_chord.take() {
        Some(prefix) => format!("{prefix} {typed}"),
        None if typed == "g" => {
            s.pending_chord = Some('g');
            return true;
        }
        None => typed,
    };
    match COMMANDS.iter().find(|c| c.keys.contains(&sequence.as_str())) {
        Some(command) => {
            (command.run)(s, tx);
            true
        }
        None => false,
    }
}

/// A key event as the keymap spells it.
fn describe(ev: &KeyEvent) -> Option<String> {
    let m = ev.mods;
    let name = match &ev.key {
        Key::Text(t) if t == " " => "space".to_string(),
        Key::Text(t) => {
            let lower = t.to_lowercase();
            return Some(match (m.shift && lower != *t, m.alt) {
                (true, _) => format!("shift+{lower}"),
                (_, true) => format!("alt+{lower}"),
                _ => t.clone(),
            });
        }
        Key::Command(c) => return Some(format!("mod+{}{c}", if m.shift { "shift+" } else { "" })),
        Key::Enter => "enter".into(),
        Key::Escape => "escape".into(),
        Key::Down => return Some("j".into()),
        Key::Up => return Some("k".into()),
        _ => return None,
    };
    Some(name)
}

pub fn palette_entries(s: &Ui) -> Vec<(Entry, String, String)> {
    let commands = COMMANDS
        .iter()
        .enumerate()
        .filter(|(_, c)| c.id != "palette")
        .map(|(i, c)| (Entry::Command(i), c.title.to_string(), c.keys.first().copied().unwrap_or("").to_string()));
    const ROLES_WITH_COMMANDS: [&str; 6] = ["inbox", "archive", "sent", "drafts", "trash", "junk"];
    let mailboxes = s.mailboxes.iter().filter(|m| !m.role.as_deref().is_some_and(|r| ROLES_WITH_COMMANDS.contains(&r))).map(|m| (Entry::Mailbox(m.id.clone()), format!("Go to {}", m.name), String::new()));
    commands.chain(mailboxes).collect()
}

pub fn run_entry(s: &mut Ui, entry: &Entry, tx: &mut Tx) {
    s.palette = None;
    match entry {
        Entry::Command(i) => (COMMANDS[*i].run)(s, tx),
        Entry::Mailbox(id) => s.show_mailbox(id),
    }
}

fn open_palette(s: &mut Ui, _: &mut Tx) {
    let mut query = TextEdit::new("");
    query.focused = true;
    s.palette = Some(Palette { query, picked: 0 });
}

fn open_search(s: &mut Ui, _: &mut Tx) {
    let mut query = TextEdit::new("");
    query.focused = true;
    s.search = Some(Search { query, results: None, picked: 0, open_when_ready: false });
}

fn next(s: &mut Ui, delta: isize) {
    match s.thread.as_mut() {
        Some(_) => {
            s.move_selection(delta);
            s.open_selected();
        }
        None => s.move_selection(delta),
    }
}

fn back(s: &mut Ui, _: &mut Tx) {
    if s.web.is_some() {
        s.close_web();
    } else {
        s.close_thread();
    }
}

fn go_role(s: &mut Ui, role: &str) {
    match s.mailbox_with_role(role).map(|m| m.id.clone()) {
        Some(id) => s.show_mailbox(&id),
        None => s.toast(format!("No {role} mailbox")),
    }
}

/// A message id and the mailboxes it is in.
type Placed = (String, Vec<String>);

/// The target conversation and its messages with their mailboxes.
fn target_messages(s: &Ui) -> Option<(String, Vec<Placed>)> {
    let thread_id = s.target_thread()?;
    let messages = s.reader.thread_message_ids(&thread_id).ok()?;
    Some((thread_id, messages))
}

/// Moves every message of the target conversation out of the current mailbox into `role`'s
/// mailbox. Messages in the current mailbox move; the rest stay where they are.
fn move_to_role(s: &mut Ui, role: &str, done: &str) {
    let Some(to) = s.mailbox_with_role(role).map(|m| m.id.clone()) else {
        s.toast(format!("No {role} mailbox"));
        return;
    };
    let Some((thread_id, messages)) = target_messages(s) else { return };
    let from = s.mailbox_id.clone();
    let ids: Vec<String> = messages.iter().filter(|(_, boxes)| boxes.contains(&from)).map(|(id, _)| id.clone()).collect();
    if ids.is_empty() || from == to {
        return;
    }
    s.drop_row(&thread_id);
    s.perform(vec![Action::Move { message_ids: ids, from: Some(from), to }], true);
    s.toast(format!("{done} · z to undo"));
}

fn archive(s: &mut Ui, _: &mut Tx) {
    move_to_role(s, "archive", "Done");
}

fn spam(s: &mut Ui, _: &mut Tx) {
    move_to_role(s, "junk", "Marked as spam");
}

/// Trash takes every message of the conversation, wherever it is, one move per mailbox so undo
/// puts each message back in all of its mailboxes.
fn trash(s: &mut Ui, _: &mut Tx) {
    let Some(to) = s.mailbox_with_role("trash").map(|m| m.id.clone()) else { return };
    let Some((thread_id, messages)) = target_messages(s) else { return };
    let by_mailbox: BTreeMap<String, Vec<String>> =
        messages.iter().fold(BTreeMap::new(), |mut acc, (id, boxes)| {
            boxes.iter().filter(|b| **b != to).for_each(|b| acc.entry(b.clone()).or_default().push(id.clone()));
            acc
        });
    let actions: Vec<Action> = by_mailbox
        .into_iter()
        .map(|(from, message_ids)| Action::Move { message_ids, from: Some(from), to: to.clone() })
        .collect();
    if actions.is_empty() {
        return;
    }
    s.drop_row(&thread_id);
    s.perform(actions, true);
    s.toast("Moved to trash · z to undo");
}

fn toggle_star(s: &mut Ui, _: &mut Tx) {
    let Some((thread_id, messages)) = target_messages(s) else { return };
    let starred = s.rows.iter().find(|r| r.thread_id == thread_id).is_some_and(|r| r.flagged);
    let target = match &s.thread {
        Some(t) => t.messages.get(t.focused).map(|m| vec![m.id.clone()]),
        None => messages.last().map(|(id, _)| vec![id.clone()]),
    };
    let Some(message_ids) = target else { return };
    let message_ids = if starred { messages.into_iter().map(|(id, _)| id).collect() } else { message_ids };
    s.perform(vec![Action::SetFlag { message_ids, flag: "$flagged".into(), on: !starred }], true);
    if let Some(row) = s.rows.iter_mut().find(|r| r.thread_id == thread_id) {
        row.flagged = !starred;
        s.rows_version += 1;
    }
}

fn set_seen(s: &mut Ui, on: bool) {
    let Some((thread_id, messages)) = target_messages(s) else { return };
    let message_ids = messages.into_iter().map(|(id, _)| id).collect();
    s.perform(vec![Action::SetFlag { message_ids, flag: "$seen".into(), on }], true);
    if !on {
        s.close_thread();
    }
    if let Some(row) = s.rows.iter_mut().find(|r| r.thread_id == thread_id) {
        row.unread_count = if on { 0 } else { row.message_count };
        s.rows_version += 1;
    }
}

fn undo(s: &mut Ui, _: &mut Tx) {
    match s.undo.pop() {
        Some(actions) => {
            s.perform(actions, false);
            s.toast("Undone");
        }
        None => s.toast("Nothing to undo"),
    }
}

fn toggle_web(s: &mut Ui, _: &mut Tx) {
    if s.web.is_some() {
        s.close_web();
        return;
    }
    let Some(thread) = s.thread.as_ref() else {
        s.open_selected();
        return toggle_web_after_open(s);
    };
    let Some(message) = thread.messages.get(thread.focused) else { return };
    match thread.bodies.get(&message.id).and_then(|r| r.document.as_ref()) {
        Some(_) => s.web = Some(message.id.clone()),
        None => s.toast("This message has no HTML version"),
    }
}

fn toggle_web_after_open(s: &mut Ui) {
    let focused = s.thread.as_ref().and_then(|t| {
        let m = t.messages.get(t.focused)?;
        t.bodies.get(&m.id)?.document.as_ref().map(|_| m.id.clone())
    });
    s.web = focused;
}

fn expand_all(s: &mut Ui, _: &mut Tx) {
    if let Some(thread) = s.thread.as_mut() {
        let all = thread.expanded.iter().all(|e| *e);
        thread.expanded.iter_mut().for_each(|e| *e = !all);
    }
}

fn enable_semantic(s: &mut Ui, _: &mut Tx) {
    if let Some(engine) = &s.engine {
        engine.send(EngineCommand::EnableSemantic);
        s.toast("Preparing meaning search in the background");
    }
}

fn enable_media(s: &mut Ui, _: &mut Tx) {
    if let Some(engine) = &s.engine {
        engine.send(EngineCommand::EnableMediaSearch);
        s.toast("Preparing image search in the background");
    }
}

fn refresh(s: &mut Ui, _: &mut Tx) {
    if let Some(engine) = &s.engine {
        engine.send(EngineCommand::Refresh);
    }
}
