//! Every command the app has, in one table: the keymap and the command palette both read it, so
//! each action has one name, one shortcut and one implementation.

use std::collections::BTreeMap;

use rune::{Key, KeyEvent, TextEdit, Tx};
use schlau_store::Action;
use schlau_sync::Command as EngineCommand;

use crate::state::{Palette, PalettePurpose, Search, Ui};

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
    Command { id: "down", title: "Down: scroll the open conversation, or the next one", keys: &["j"], run: |s, tx| down(s, Scroll::Lines(1.0), tx) },
    Command { id: "up", title: "Up: scroll the open conversation, or the previous one", keys: &["k"], run: |s, tx| down(s, Scroll::Lines(-1.0), tx) },
    Command { id: "next", title: "Next conversation", keys: &["shift+j"], run: |s, _| next(s, 1) },
    Command { id: "previous", title: "Previous conversation", keys: &["shift+k"], run: |s, _| next(s, -1) },
    Command { id: "next-message", title: "Next message in the conversation", keys: &["n"], run: |s, tx| step_message(s, 1, tx) },
    Command { id: "previous-message", title: "Previous message in the conversation", keys: &["p"], run: |s, tx| step_message(s, -1, tx) },
    Command { id: "half-page-down", title: "Half a page down", keys: &["ctrl+d"], run: |s, tx| down(s, Scroll::Pages(0.5), tx) },
    Command { id: "half-page-up", title: "Half a page up", keys: &["ctrl+u"], run: |s, tx| down(s, Scroll::Pages(-0.5), tx) },
    Command { id: "page-down", title: "Page down", keys: &["pagedown", "ctrl+f"], run: |s, tx| down(s, Scroll::Pages(1.0), tx) },
    Command { id: "page-up", title: "Page up", keys: &["pageup", "ctrl+b"], run: |s, tx| down(s, Scroll::Pages(-1.0), tx) },
    Command { id: "next-period", title: "Next period (today, yesterday, this week, this month, then by month)", keys: &["] d", "] ]"], run: |s, tx| jump_period(s, 1, tx) },
    Command { id: "previous-period", title: "Previous period, or the start of this one", keys: &["[ d", "[ ["], run: |s, tx| jump_period(s, -1, tx) },
    Command { id: "top", title: "Top: start of the conversation, or the newest one", keys: &["g g", "home"], run: |s, tx| down(s, Scroll::Top, tx) },
    Command { id: "bottom", title: "Bottom: end of the conversation, or the oldest loaded one", keys: &["shift+g", "end"], run: |s, tx| down(s, Scroll::Bottom, tx) },
    Command { id: "open", title: "Open conversation", keys: &["enter", "o", "l"], run: |s, _| s.open_selected() },
    Command { id: "back", title: "Back", keys: &["escape", "u", "h"], run: back },
    Command { id: "settings", title: "Settings", keys: &["mod+,"], run: |s, _| crate::settings_pane::toggle(s) },
    Command { id: "folders", title: "Go to folder…", keys: &["g f", "g l"], run: open_folders },
    Command { id: "screener", title: "Screener: decide on new senders", keys: &["g s"], run: |s, _| crate::screener::open(s) },
    Command { id: "screener-setup", title: "Screener setup: folder, destinations and Fastmail rules", keys: &[], run: |s, _| crate::screener::open_setup(s) },
    Command { id: "quick-look", title: "Quick look attachments", keys: &["space"], run: quick_look },
    Command { id: "archive", title: "Mark done (archive)", keys: &["e"], run: archive },
    Command { id: "trash", title: "Move to trash", keys: &["#"], run: trash },
    Command { id: "spam", title: "Mark as spam", keys: &["!"], run: spam },
    Command { id: "star", title: "Star / unstar", keys: &["s"], run: toggle_star },
    Command { id: "unread", title: "Mark unread", keys: &["shift+u"], run: |s, _| set_seen(s, false) },
    Command { id: "read", title: "Mark read", keys: &["shift+i"], run: |s, _| set_seen(s, true) },
    Command { id: "undo", title: "Undo", keys: &["z", "mod+z"], run: undo },
    Command { id: "load-images", title: "Load images in the conversation", keys: &["i"], run: |s, _| s.load_images(s.open_images()) },
    Command { id: "web", title: "View original (secure web view)", keys: &["v"], run: toggle_web },
    Command { id: "expand", title: "Expand all messages", keys: &["shift+e"], run: expand_all },
    Command { id: "refresh", title: "Sync now", keys: &["mod+r"], run: refresh },
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

/// Keys that start a two-key chord: `g i`, `] d`.
const CHORD_PREFIXES: [char; 3] = ['g', '[', ']'];

/// Finds the command bound to a key event, handling two-key chords.
pub fn dispatch(s: &mut Ui, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let typed = describe(ev);
    let Some(typed) = typed else { return false };
    let prefix = typed.chars().next().filter(|c| typed.len() == 1 && CHORD_PREFIXES.contains(c));
    let sequence = match (s.pending_chord.take(), prefix) {
        (Some(pending), _) => format!("{pending} {typed}"),
        (None, Some(prefix)) => {
            s.pending_chord = Some(prefix);
            return true;
        }
        (None, None) => typed,
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
            return Some(match (m.ctrl && !m.cmd, m.shift && (lower != *t || lower.chars().all(char::is_alphabetic)), m.alt) {
                (true, _, _) => format!("ctrl+{lower}"),
                (_, true, _) => format!("shift+{lower}"),
                (_, _, true) => format!("alt+{lower}"),
                _ => t.clone(),
            });
        }
        Key::Command(c) => return Some(format!("mod+{}{c}", if m.shift { "shift+" } else { "" })),
        Key::Enter => "enter".into(),
        Key::Escape => "escape".into(),
        Key::PageDown => "pagedown".into(),
        Key::PageUp => "pageup".into(),
        Key::Home => "home".into(),
        Key::End => "end".into(),
        Key::Down => return Some("j".into()),
        Key::Up => return Some("k".into()),
        Key::Left => return Some("h".into()),
        Key::Right => return Some("l".into()),
        _ => return None,
    };
    Some(name)
}

/// A folder's name with its parents, `Parent / Child`.
fn folder_path(s: &Ui, m: &schlau_store::Mailbox) -> String {
    let parent = m.parent_id.as_ref().and_then(|p| s.mailboxes.iter().find(|b| &b.id == p));
    match parent {
        Some(parent) if parent.id != m.id => format!("{} / {}", folder_path(s, parent), m.name),
        _ => m.name.clone(),
    }
}

fn unread_hint(m: &schlau_store::Mailbox) -> String {
    if m.unread_threads > 0 { m.unread_threads.to_string() } else { String::new() }
}

/// Palette entries: a title and a hint (the keys of a command, the unread count of a folder).
pub fn palette_entries(s: &Ui) -> Vec<(Entry, String, String)> {
    if s.palette.as_ref().is_some_and(|p| p.purpose.lists_folders()) {
        return s.mailboxes.iter().map(|m| (Entry::Mailbox(m.id.clone()), folder_path(s, m), unread_hint(m))).collect();
    }
    let commands = COMMANDS
        .iter()
        .enumerate()
        .filter(|(_, c)| c.id != "palette")
        .map(|(i, c)| (Entry::Command(i), c.title.to_string(), c.keys.first().copied().unwrap_or("").to_string()));
    const ROLES_WITH_COMMANDS: [&str; 6] = ["inbox", "archive", "sent", "drafts", "trash", "junk"];
    let mailboxes = s.mailboxes.iter().filter(|m| !m.role.as_deref().is_some_and(|r| ROLES_WITH_COMMANDS.contains(&r))).map(|m| (Entry::Mailbox(m.id.clone()), format!("Go to {}", folder_path(s, m)), unread_hint(m)));
    commands.chain(mailboxes).collect()
}

pub fn run_entry(s: &mut Ui, entry: &Entry, tx: &mut Tx) {
    let purpose = s.palette.take().map_or(PalettePurpose::Commands, |p| p.purpose);
    match (entry, purpose) {
        (Entry::Command(i), _) => (COMMANDS[*i].run)(s, tx),
        (Entry::Mailbox(id), PalettePurpose::Destination) => crate::screener::add_destination(s, id),
        (Entry::Mailbox(id), PalettePurpose::Triage) => crate::screener::set_triage(s, id),
        (Entry::Mailbox(id), PalettePurpose::Commands | PalettePurpose::Folders) => s.show_mailbox(id),
    }
}

/// Opens the palette for `purpose`, with an empty query.
pub fn open_palette_for(s: &mut Ui, purpose: PalettePurpose) {
    let mut query = TextEdit::new("");
    query.focused = true;
    s.palette = Some(Palette { query, picked: 0, purpose });
}

fn open_palette(s: &mut Ui, _: &mut Tx) {
    open_palette_for(s, PalettePurpose::Commands);
}

fn open_folders(s: &mut Ui, _: &mut Tx) {
    open_palette_for(s, PalettePurpose::Folders);
}

/// Quick look at the focused message's attachments, or the nearest earlier message's that has some.
fn quick_look(s: &mut Ui, _: &mut Tx) {
    let Some(t) = s.thread.as_ref() else {
        s.toast("Open a conversation to look at its attachments");
        return;
    };
    let found = t.messages[..=t.focused.min(t.messages.len().saturating_sub(1))]
        .iter()
        .rev()
        .chain(t.messages.iter().skip(t.focused + 1))
        .filter_map(|m| t.bodies.get(&m.id))
        .map(|r| r.attachments.clone())
        .find(|a| !a.is_empty())
        .unwrap_or_default();
    s.quick_look(found, 0);
}

fn open_search(s: &mut Ui, _: &mut Tx) {
    let mut query = TextEdit::new("");
    query.focused = true;
    s.search = Some(Search { query, results: None, picked: 0, open_when_ready: false });
}

/// Scrolls the list so row `key` sits at its top, its period heading first. Revealing mounts a
/// row that is far away; the next frame places it.
pub fn reveal_at_top(s: &mut Ui, key: String, tx: &mut Tx) {
    tx.scroll_into_view(key.clone());
    s.pending_top = Some(key);
    tx.wake_after(std::time::Duration::from_millis(16));
}

/// `]d` and `[d`: the first conversation of the next period, or back to the start of this one. In
/// an open conversation, opens it.
fn jump_period(s: &mut Ui, delta: isize, tx: &mut Tx) {
    let Some(key) = s.jump_period(delta) else { return };
    match s.thread {
        Some(_) => s.open_selected(),
        None => reveal_at_top(s, key, tx),
    }
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

/// How far a movement key goes: in the open conversation, a scroll; in the list, rows.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Scroll {
    Lines(f32),
    Pages(f32),
    Top,
    Bottom,
}

/// A line step: about three lines of body text.
const SCROLL_LINE: f32 = 60.0;
/// Rows a page moves the selection in the list.
const LIST_PAGE: f32 = 20.0;
/// Within this of the last scroll key, the next one adds to its goal: key repeat keeps gliding.
const SCROLL_REPEAT: std::time::Duration = std::time::Duration::from_millis(300);
const SCROLL_SPRING: rune::Spring = rune::Spring::new(0.18, 0.0);

/// The reader offset `by` asks for, from `base`, for a viewport at `at`.
fn scroll_target(by: Scroll, base: f32, at: &rune::ScrollPos) -> f32 {
    let target = match by {
        Scroll::Lines(n) => base + n * SCROLL_LINE,
        Scroll::Pages(n) => base + n * at.viewport[1] * 0.9,
        Scroll::Top => 0.0,
        Scroll::Bottom => at.max[1],
    };
    target.clamp(0.0, at.max[1].max(0.0))
}

/// Scrolls the open conversation; in the list, moves the selection.
fn down(s: &mut Ui, by: Scroll, tx: &mut Tx) {
    if s.web.is_some() {
        return;
    }
    let Some(t) = s.thread.as_mut() else {
        let rows = match by {
            Scroll::Lines(n) => n,
            Scroll::Pages(n) => n * LIST_PAGE,
            Scroll::Top => f32::MIN / 2.0,
            Scroll::Bottom => f32::MAX / 2.0,
        };
        s.move_selection(rows.clamp(isize::MIN as f32 / 2.0, isize::MAX as f32 / 2.0) as isize);
        return;
    };
    let at = t.scroll.get();
    let now = std::time::Instant::now();
    let base = t.scroll_goal.filter(|(_, when)| now - *when < SCROLL_REPEAT).map_or(at.offset[1], |(y, _)| y);
    let target = scroll_target(by, base, &at);
    t.scroll_goal = Some((target, now));
    tx.scroll_to_with("thread", 0.0, target, SCROLL_SPRING);
}

/// Focuses the next or previous message of the open conversation, opens it and brings its top in.
fn step_message(s: &mut Ui, delta: isize, tx: &mut Tx) {
    let Some(t) = s.thread.as_mut().filter(|t| !t.messages.is_empty()) else { return };
    let last = t.messages.len() as isize - 1;
    t.focused = (t.focused as isize + delta).clamp(0, last) as usize;
    if let Some(open) = t.expanded.get_mut(t.focused) {
        *open = true;
    }
    t.scroll_goal = None;
    tx.scroll_key_to(format!("m:{}", t.messages[t.focused].id), 12.0);
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



fn refresh(s: &mut Ui, _: &mut Tx) {
    if let Some(engine) = &s.engine {
        engine.send(EngineCommand::Refresh);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::Mods;

    fn key(text: &str, mods: Mods) -> Option<String> {
        describe(&KeyEvent { key: Key::Text(text.into()), mods })
    }

    #[test]
    fn keys_are_named_as_the_keymap_spells_them() {
        assert_eq!(key("j", Mods::default()).as_deref(), Some("j"));
        assert_eq!(key("J", Mods { shift: true, ..Mods::default() }).as_deref(), Some("shift+j"));
        assert_eq!(key("j", Mods { shift: true, ..Mods::default() }).as_deref(), Some("shift+j"));
        assert_eq!(key("#", Mods { shift: true, ..Mods::default() }).as_deref(), Some("#"));
        assert_eq!(key("d", Mods { ctrl: true, ..Mods::default() }).as_deref(), Some("ctrl+d"), "not archive's e or a plain d");
        assert_eq!(describe(&Key::PageDown.into()).as_deref(), Some("pagedown"));
        assert_eq!(key("]", Mods::default()).as_deref(), Some("]"));
    }

    #[test]
    fn brackets_start_chords_for_periods() {
        let ids = |keys: &str| COMMANDS.iter().filter(|c| c.keys.contains(&keys)).map(|c| c.id).collect::<Vec<_>>();
        assert_eq!(ids("] d"), ["next-period"]);
        assert_eq!(ids("[ ["), ["previous-period"]);
        let prefixes: std::collections::BTreeSet<char> =
            COMMANDS.iter().flat_map(|c| c.keys.iter()).filter_map(|k| k.split_once(' ')).filter_map(|(p, _)| p.chars().next()).collect();
        assert!(prefixes.iter().all(|p| CHORD_PREFIXES.contains(p)), "{prefixes:?}");
        assert!(COMMANDS.iter().flat_map(|c| c.keys.iter()).all(|k| !CHORD_PREFIXES.iter().any(|p| *k == p.to_string())), "a prefix alone is no command");
    }

    #[test]
    fn every_key_sequence_has_one_command() {
        let mut keys: Vec<&str> = COMMANDS.iter().flat_map(|c| c.keys.iter().copied()).collect();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count);
    }

    #[test]
    fn scroll_targets_stay_inside_the_message() {
        let mut at = rune::ScrollPos::default();
        (at.offset, at.max, at.viewport) = ([0.0, 100.0], [0.0, 1000.0], [800.0, 400.0]);
        assert_eq!(scroll_target(Scroll::Lines(1.0), 100.0, &at), 100.0 + SCROLL_LINE);
        assert_eq!(scroll_target(Scroll::Lines(-3.0), 100.0, &at), 0.0);
        assert_eq!(scroll_target(Scroll::Pages(0.5), 100.0, &at), 280.0);
        assert_eq!(scroll_target(Scroll::Pages(9.0), 100.0, &at), 1000.0);
        assert_eq!(scroll_target(Scroll::Bottom, 100.0, &at), 1000.0);
        assert_eq!(scroll_target(Scroll::Top, 500.0, &at), 0.0);
    }
}
