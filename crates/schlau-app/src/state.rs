use std::collections::HashMap;
use std::path::PathBuf;

use rune::{Latest, TextEdit};
use schlau_store::{Action, Mailbox, MessageView, PageCursor, Reader, Store, ThreadRow};
use schlau_sync::{AttachmentWorker, Command, Connect, EngineHandle, Notify, SharedEmbedder, SharedMedia, Status};

use crate::search::SearchResults;

pub const PAGE: usize = 400;

pub struct Ui {
    pub db_path: PathBuf,
    pub reader: Reader,
    pub pending_store: Option<Store>,
    pub pending_connect: Option<Connect>,
    pub has_account: bool,
    pub embedder: SharedEmbedder,
    /// The multimodal model for image and attachment search, once loaded.
    pub media: SharedMedia,
    /// Whether the open search already used the multimodal model; it may load after the box opens.
    pub media_ready: bool,
    pub pending_worker: Option<AttachmentWorker>,
    pub engine: Option<EngineHandle>,
    pub notify: Option<Notify>,
    pub seen_version: u64,
    pub status: Status,
    /// Work beside sync, such as indexing attachments.
    pub background: Option<String>,

    pub mailboxes: Vec<Mailbox>,
    pub mailbox_id: String,
    pub rows: Vec<ThreadRow>,
    pub rows_version: u64,
    pub list_complete: bool,
    pub selected: usize,

    pub thread: Option<OpenThread>,
    /// The message shown in the secure web view, when it's open.
    pub web: Option<String>,
    pub palette: Option<Palette>,
    pub search: Option<Search>,
    pub search_results: Latest<SearchResults>,
    pub pending_chord: Option<char>,
    pub undo: Vec<Vec<Action>>,
    pub toast: Option<Toast>,
    pub clock: f64,
    /// An element to bring into view on the next frame, once the view that holds it is built.
    pub pending_scroll: Option<String>,
}

pub struct OpenThread {
    pub thread_id: String,
    pub messages: Vec<MessageView>,
    pub bodies: HashMap<String, crate::reader::Rendered>,
    pub expanded: Vec<bool>,
    pub focused: usize,
}

pub struct Palette {
    pub query: TextEdit,
    pub picked: usize,
}

pub struct Search {
    pub query: TextEdit,
    pub results: Option<SearchResults>,
    pub picked: usize,
    /// Return was pressed before the answer for the current query landed.
    pub open_when_ready: bool,
}

pub struct Toast {
    pub text: String,
    pub until: f64,
}

impl Ui {
    pub fn new(db_path: PathBuf, store: Store, connect: Option<Connect>) -> Ui {
        let reader = store.reader().expect("database opens for reading");
        let mut ui = Ui {
            db_path,
            reader,
            pending_store: Some(store),
            has_account: connect.is_some(),
            embedder: SharedEmbedder::default(),
            media: SharedMedia::default(),
            media_ready: false,
            pending_worker: None,
            pending_connect: connect,
            engine: None,
            notify: None,
            seen_version: 0,
            status: Status::Idle,
            background: None,
            mailboxes: Vec::new(),
            mailbox_id: String::new(),
            rows: Vec::new(),
            rows_version: 0,
            list_complete: false,
            selected: 0,
            thread: None,
            web: None,
            palette: None,
            search: None,
            search_results: Latest::default().debounce(std::time::Duration::from_millis(30)),
            pending_chord: None,
            undo: Vec::new(),
            toast: None,
            clock: 0.0,
            pending_scroll: None,
        };
        ui.mailboxes = ui.reader.mailboxes().unwrap_or_default();
        let inbox = ui.mailbox_with_role("inbox").or_else(|| ui.mailboxes.first()).map(|m| m.id.clone());
        if let Some(inbox) = inbox {
            ui.show_mailbox(&inbox);
        }
        ui
    }

    pub fn mailbox_with_role(&self, role: &str) -> Option<&Mailbox> {
        self.mailboxes.iter().find(|m| m.role.as_deref() == Some(role))
    }

    pub fn current_mailbox(&self) -> Option<&Mailbox> {
        self.mailboxes.iter().find(|m| m.id == self.mailbox_id)
    }

    pub fn show_mailbox(&mut self, id: &str) {
        self.mailbox_id = id.to_string();
        self.thread = None;
        self.selected = 0;
        self.rows = self.reader.thread_page(id, None, PAGE).unwrap_or_default();
        self.list_complete = self.rows.len() < PAGE;
        self.rows_version += 1;
    }

    /// Loads the next page when the visible rows come near the end of what's loaded.
    pub fn load_more(&mut self) -> bool {
        if self.list_complete {
            return false;
        }
        let Some(last) = self.rows.last() else { return false };
        let cursor = PageCursor { latest_at: last.latest_at, thread_id: &last.thread_id };
        let more = self.reader.thread_page(&self.mailbox_id, Some(cursor), PAGE).unwrap_or_default();
        self.list_complete = more.len() < PAGE;
        self.rows.extend(more);
        self.rows_version += 1;
        true
    }

    /// Re-reads what's on screen after the engine committed a change, keeping the selection on the
    /// same conversation when it's still listed.
    pub fn reload(&mut self) {
        self.mailboxes = self.reader.mailboxes().unwrap_or_default();
        let keep = self.selected_row().map(|r| r.thread_id.clone());
        let count = self.rows.len().max(PAGE);
        self.rows = self.reader.thread_page(&self.mailbox_id, None, count).unwrap_or_default();
        self.list_complete = self.rows.len() < count;
        if let Some(keep) = keep
            && let Some(index) = self.rows.iter().position(|r| r.thread_id == keep)
        {
            self.selected = index;
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.rows_version += 1;
        if let Some(thread_id) = self.thread.as_ref().map(|t| t.thread_id.clone()) {
            self.refresh_thread(&thread_id);
        }
    }

    pub fn selected_row(&self) -> Option<&ThreadRow> {
        self.rows.get(self.selected)
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.rows.is_empty() {
            return;
        }
        let last = self.rows.len() as isize - 1;
        self.selected = (self.selected as isize + delta).clamp(0, last) as usize;
        if self.selected + 50 > self.rows.len() {
            self.load_more();
        }
    }

    pub fn select_thread(&mut self, thread_id: &str) {
        if let Some(index) = self.rows.iter().position(|r| r.thread_id == thread_id) {
            self.selected = index;
        }
    }

    pub fn open_selected(&mut self) {
        let Some(thread_id) = self.selected_row().map(|r| r.thread_id.clone()) else { return };
        self.open_thread(&thread_id);
    }

    pub fn open_thread(&mut self, thread_id: &str) {
        let messages = self.reader.thread_messages(thread_id).unwrap_or_default();
        let unread: Vec<String> = messages
            .iter()
            .filter(|m| !m.flags.contains(schlau_store::Flags::SEEN))
            .map(|m| m.id.clone())
            .collect();
        let expanded = messages
            .iter()
            .enumerate()
            .map(|(i, m)| i + 1 == messages.len() || !m.flags.contains(schlau_store::Flags::SEEN))
            .collect();
        let focused = messages
            .iter()
            .position(|m| !m.flags.contains(schlau_store::Flags::SEEN))
            .unwrap_or(messages.len().saturating_sub(1));
        self.pending_scroll = messages.get(focused).map(|m| format!("m:{}", m.id));
        self.thread =
            Some(OpenThread { thread_id: thread_id.to_string(), messages, bodies: HashMap::new(), expanded, focused });
        self.load_bodies();
        if !unread.is_empty() {
            self.perform(vec![Action::SetFlag { message_ids: unread, flag: "$seen".into(), on: true }], false);
        }
    }

    fn refresh_thread(&mut self, thread_id: &str) {
        let messages = self.reader.thread_messages(thread_id).unwrap_or_default();
        if messages.is_empty() {
            self.thread = None;
            return;
        }
        if let Some(thread) = self.thread.as_mut() {
            if thread.messages.len() != messages.len() {
                thread.expanded.resize(messages.len(), true);
                thread.focused = thread.focused.min(messages.len() - 1);
            }
            thread.messages = messages;
        }
        self.load_bodies();
    }

    /// Reads the bodies the open conversation has locally and asks the engine for the rest.
    fn load_bodies(&mut self) {
        let Some(thread) = self.thread.as_mut() else { return };
        let wanted: Vec<String> =
            thread.messages.iter().filter(|m| !thread.bodies.contains_key(&m.id)).map(|m| m.id.clone()).collect();
        let missing: Vec<String> = wanted
            .into_iter()
            .filter(|id| match self.reader.body(id) {
                Ok(Some(body)) => {
                    thread.bodies.insert(id.clone(), crate::reader::render(&body));
                    false
                }
                _ => true,
            })
            .collect();
        if !missing.is_empty()
            && let Some(engine) = &self.engine
        {
            engine.send(Command::FetchBodies(missing));
        }
    }

    pub fn toggle_message(&mut self, message_id: &str) {
        if let Some(thread) = self.thread.as_mut()
            && let Some(index) = thread.messages.iter().position(|m| m.id == message_id)
        {
            thread.expanded[index] = !thread.expanded[index];
            thread.focused = index;
        }
    }

    pub fn close_web(&mut self) {
        if self.web.take().is_some() {
            crate::webview::hide();
        }
    }

    pub fn close_thread(&mut self) {
        self.close_web();
        self.thread = None;
    }

    /// The conversation an action applies to: the open one, else the selected row.
    pub fn target_thread(&self) -> Option<String> {
        self.thread.as_ref().map(|t| t.thread_id.clone()).or_else(|| self.selected_row().map(|r| r.thread_id.clone()))
    }

    /// Sends actions to the engine. With `undoable`, their inverse goes on the undo stack.
    pub fn perform(&mut self, actions: Vec<Action>, undoable: bool) {
        if undoable {
            self.undo.push(actions.iter().rev().map(inverse).collect());
            if self.undo.len() > 50 {
                self.undo.remove(0);
            }
        }
        if let Some(engine) = &self.engine {
            actions.into_iter().for_each(|a| engine.send(Command::Perform(a)));
        }
    }

    pub fn request_search(&mut self) {
        let Some(search) = self.search.as_mut() else { return };
        search.picked = 0;
        let query = search.query.text.clone();
        if query.trim().is_empty() {
            search.results = None;
            self.search_results.cancel();
            return;
        }
        let request = crate::search::SearchRequest {
            path: self.db_path.clone(),
            query,
            version: self.seen_version,
            embedder: self.embedder.get(),
            media: self.media.get(),
        };
        self.search_results.request(request, crate::search::run);
    }

    pub fn open_search_result(&mut self) {
        let waiting = self.search.as_ref().is_some_and(|s| {
            s.results.as_ref().is_none_or(|r| r.query != s.query.text) && !s.query.text.trim().is_empty()
        });
        if waiting {
            if let Some(search) = self.search.as_mut() {
                search.open_when_ready = true;
            }
            return;
        }
        let Some(search) = self.search.take() else { return };
        let picked = search.results.and_then(|r| r.rows.into_iter().nth(search.picked));
        if let Some(row) = picked {
            self.open_thread(&row.thread_id);
        }
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some(Toast { text: text.into(), until: self.clock + 2.5 });
    }

    /// Removes the conversation from the list at once, before the engine confirms, and keeps the
    /// selection on the next row, as Superhuman does after Done.
    pub fn drop_row(&mut self, thread_id: &str) {
        if let Some(index) = self.rows.iter().position(|r| r.thread_id == thread_id) {
            self.rows.remove(index);
            self.rows_version += 1;
            self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        }
        if self.thread.as_ref().is_some_and(|t| t.thread_id == thread_id) {
            self.thread = None;
        }
    }
}

/// The action that undoes `action`. Destroy can't be undone; it is never queued as undoable.
fn inverse(action: &Action) -> Action {
    match action {
        Action::SetFlag { message_ids, flag, on } => {
            Action::SetFlag { message_ids: message_ids.clone(), flag: flag.clone(), on: !on }
        }
        Action::Move { message_ids, from: Some(from), to } => {
            Action::Move { message_ids: message_ids.clone(), from: Some(to.clone()), to: from.clone() }
        }
        Action::Move { message_ids, from: None, to } => {
            Action::RemoveMailbox { message_ids: message_ids.clone(), mailbox_id: to.clone() }
        }
        Action::AddMailbox { message_ids, mailbox_id } => {
            Action::RemoveMailbox { message_ids: message_ids.clone(), mailbox_id: mailbox_id.clone() }
        }
        Action::RemoveMailbox { message_ids, mailbox_id } => {
            Action::AddMailbox { message_ids: message_ids.clone(), mailbox_id: mailbox_id.clone() }
        }
        Action::Destroy { message_ids } => Action::Destroy { message_ids: message_ids.clone() },
    }
}
