//! The settings pane: every setting on one page, moved through with j/k and changed with h/l, and the
//! embeddings server's address typed in place.

use rune::{Key, KeyEvent, TextEdit, Tx};

use crate::settings::{Choice, Setting};
use crate::state::Ui;

/// A row of the pane, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneRow {
    Setting(Setting),
    ServerUrl,
    ServerModel,
}

pub const ROWS: [PaneRow; 7] = [
    PaneRow::Setting(Setting::Typos),
    PaneRow::Setting(Setting::LastWord),
    PaneRow::Setting(Setting::TextMeaning),
    PaneRow::Setting(Setting::AttachmentMeaning),
    PaneRow::ServerUrl,
    PaneRow::ServerModel,
    PaneRow::Setting(Setting::Preview),
];

pub struct SettingsPane {
    pub picked: usize,
    pub url: TextEdit,
    pub model: TextEdit,
    /// A Server choice made before there was a server: applied once one is saved.
    pub waiting: Option<Choice>,
    /// A field to focus on the next frame, once the key that asked for it is spent: focused at
    /// once, the field would also type that key.
    pub focus_next: Option<PaneRow>,
}

impl SettingsPane {
    pub fn row(&self) -> PaneRow {
        ROWS[self.picked.min(ROWS.len() - 1)]
    }

    fn field(&mut self, row: PaneRow) -> Option<&mut TextEdit> {
        match row {
            PaneRow::ServerUrl => Some(&mut self.url),
            PaneRow::ServerModel => Some(&mut self.model),
            PaneRow::Setting(_) => None,
        }
    }
}

pub fn open(s: &mut Ui) {
    s.close_web();
    let server = s.settings.stored_server();
    let url = TextEdit::new(server.map(|s| s.url.as_str()).unwrap_or(""));
    let model = TextEdit::new(server.map(|s| s.model.as_str()).unwrap_or("embeddinggemma-2"));
    s.settings_pane = Some(SettingsPane { picked: 0, url, model, waiting: None, focus_next: None });
}

pub fn toggle(s: &mut Ui) {
    match s.settings_pane {
        Some(_) => close(s),
        None => open(s),
    }
}

pub fn close(s: &mut Ui) {
    s.settings_pane = None;
}

pub fn pick(s: &mut Ui, row: PaneRow) {
    if let Some(pane) = s.settings_pane.as_mut() {
        pane.picked = ROWS.iter().position(|&r| r == row).unwrap_or(pane.picked);
    }
}

/// Moves the picked setting `delta` choices along.
fn step_choice(s: &mut Ui, setting: Setting, delta: isize) {
    let next = setting.stepped(&s.settings.effective(), delta);
    if !next.is_current(&s.settings.effective()) {
        s.choose(next);
    }
}

/// Focuses the field on `row` from the next frame, so typing goes into it.
pub fn edit(s: &mut Ui, row: PaneRow) {
    pick(s, row);
    if let Some(pane) = s.settings_pane.as_mut() {
        pane.focus_next = Some(row);
    }
}

/// Focuses the field `edit` asked for. Called each frame; true when it changed something.
pub fn focus_pending(s: &mut Ui) -> bool {
    let Some(pane) = s.settings_pane.as_mut() else { return false };
    let Some(row) = pane.focus_next.take() else { return false };
    pane.field(row).map(|field| field.focused = true).is_some()
}

/// Saves the server fields, and tells search when a meaning setting uses the server.
pub fn save_server(s: &mut Ui, tx: &mut Tx) {
    let Some(pane) = s.settings_pane.as_mut() else { return };
    pane.url.focused = false;
    pane.model.focused = false;
    let (url, model) = (pane.url.text.clone(), pane.model.text.clone());
    tx.blur();
    let before = s.settings.stored_server().cloned();
    let saved = s.settings.set_server(&url, &model);
    if s.settings.stored_server() == before.as_ref() {
        return;
    }
    s.configure_search();
    match (saved, s.settings.stored_server()) {
        (Err(e), _) => s.toast(format!("Embeddings server not saved: {e}")),
        (Ok(()), Some(server)) => s.toast(format!("Embeddings server: {}", server.url)),
        (Ok(()), None) => s.toast("Embeddings server removed"),
    }
    let waiting = s.settings_pane.as_mut().and_then(|p| p.waiting.take());
    if let Some(choice) = waiting.filter(|_| s.settings.effective().search.server.is_some()) {
        pick(s, PaneRow::Setting(choice.setting()));
        s.choose(choice);
    }
}

/// Keys while the pane is open and no field has focus. Shortcuts with ⌘ still reach the keymap.
pub fn on_key(s: &mut Ui, ev: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(pane) = s.settings_pane.as_mut() else { return false };
    let row = pane.row();
    let text = match &ev.key {
        Key::Text(t) if !ev.mods.cmd && !ev.mods.ctrl => t.as_str(),
        Key::Down => "j",
        Key::Up => "k",
        Key::Left => "h",
        Key::Right => "l",
        Key::Enter => "enter",
        Key::Escape => "escape",
        Key::Tab if ev.mods.shift => "k",
        Key::Tab => "j",
        _ => return crate::commands::dispatch(s, ev, tx),
    };
    match (text, row) {
        ("j", _) => pane.picked = (pane.picked + 1).min(ROWS.len() - 1),
        ("k", _) => pane.picked = pane.picked.saturating_sub(1),
        ("h", PaneRow::Setting(setting)) => step_choice(s, setting, -1),
        ("l", PaneRow::Setting(setting)) => step_choice(s, setting, 1),
        ("enter" | " ", PaneRow::Setting(setting)) => {
            let choices = setting.choices();
            let current = choices.iter().position(|c| c.is_current(&s.settings.effective())).unwrap_or(0);
            s.choose(choices[(current + 1) % choices.len()]);
        }
        ("enter" | "l" | "i", field) => edit(s, field),
        ("escape" | "q" | "u" | "h", _) => close(s),
        _ => {}
    }
    if let Some(pane) = s.settings_pane.as_ref() {
        tx.scroll_into_view(crate::view::settings_row_key(pane.picked));
    }
    true
}

/// The detail line under a choice: what it does, and whether its model is on disk.
pub fn detail(s: &Ui, choice: Choice) -> String {
    let download = choice.model().map(|spec| {
        match schlau_embed::ModelStore::local(&s.models_dir, spec).is_some() {
            true => " Downloaded.",
            false => " Downloads when chosen.",
        }
    });
    format!("{}{}", choice.detail(), download.unwrap_or(""))
}
