use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use schlau_store::{Action, Store};

use std::path::PathBuf;

use crate::attachments::{AttachmentWorker, Signal};
use crate::push;
use crate::semantic::{self, SharedEmbedder};
use crate::remote::{DynClient, Remote};

/// Makes a connected client. The engine calls it on its own thread, so connecting never blocks
/// the UI, and again after a failure.
pub type Connect = Arc<dyn Fn() -> schlau_jmap::Result<DynClient> + Send + Sync>;

/// What the UI asks of the engine.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Apply a user action locally now and replay it on the server.
    Perform(Action),
    /// Fetch these messages' bodies first (the open conversation).
    FetchBodies(Vec<String>),
    /// Sync now instead of waiting for push.
    Refresh,
    /// Download the embedding model if needed and embed every message, for meaning search.
    EnableSemantic,
    /// Download the multimodal model if needed (or use the configured server) and embed attachments.
    EnableMediaSearch,
    Shutdown,
}

/// What the engine is doing, for the status line.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum Status {
    #[default]
    Idle,
    Offline,
    Connecting,
    Syncing { done: u64, total: Option<u64> },
    /// Local preparation, such as downloading or running the search model.
    Preparing(String),
    Error(String),
}

/// Wakes the UI when the replica changed. `version` grows with every committed change, so the UI
/// reloads only what it shows, and only when it moved.
#[derive(Clone)]
pub struct Notify {
    version: Arc<AtomicU64>,
    status: Arc<Mutex<Status>>,
    background: Arc<Mutex<Option<String>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Notify {
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Notify {
        Notify { version: Arc::default(), status: Arc::default(), background: Arc::default(), wake: Arc::new(wake) }
    }

    pub fn version(&self) -> u64 {
        self.version.load(Ordering::Acquire)
    }

    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Work going on beside sync, such as indexing attachments, for the status line when sync is idle.
    pub fn background(&self) -> Option<String> {
        self.background.lock().ok().and_then(|b| b.clone())
    }

    pub(crate) fn set_background(&self, note: Option<String>) {
        if let Ok(mut b) = self.background.lock()
            && *b != note
        {
            *b = note;
            (self.wake)();
        }
    }

    pub(crate) fn changed(&self) {
        self.version.fetch_add(1, Ordering::AcqRel);
        (self.wake)();
    }

    pub(crate) fn set_status(&self, status: Status) {
        if let Ok(mut s) = self.status.lock()
            && *s != status
        {
            *s = status;
            (self.wake)();
        }
    }
}

pub struct EngineHandle {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl EngineHandle {
    pub fn send(&self, command: Command) {
        let _ = self.commands.send(command);
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub struct Engine {
    store: Store,
    notify: Notify,
    connect: Option<Connect>,
    semantic: Option<(SharedEmbedder, PathBuf)>,
    attachments: Option<AttachmentWorker>,
    kick: Option<Sender<Signal>>,
}

/// Work the engine still owes the server or the replica, most urgent first.
#[derive(Debug, Default)]
struct Pending {
    sync: bool,
    bodies: Vec<String>,
    backfill: bool,
    background_bodies: bool,
    load_model: bool,
    download_model: bool,
    embed: bool,
}

const IDLE_SYNC: Duration = Duration::from_secs(300);

impl Engine {
    pub fn new(store: Store, notify: Notify) -> Engine {
        Engine { store, notify, connect: None, semantic: None, attachments: None, kick: None }
    }

    /// Embeds messages for meaning search with the model in `cache`, loaded if it was downloaded
    /// before; `Command::EnableSemantic` downloads it.
    pub fn with_semantic(mut self, shared: SharedEmbedder, cache: PathBuf) -> Engine {
        self.semantic = Some((shared, cache));
        self
    }

    /// Syncs with a JMAP account. Without it the engine keeps a local replica only.
    pub fn with_remote(mut self, connect: Connect) -> Engine {
        self.connect = Some(connect);
        self
    }

    /// Indexes attachments on a thread of their own: text for the word index, vectors for image search.
    pub fn with_attachments(mut self, worker: AttachmentWorker) -> Engine {
        self.attachments = Some(worker);
        self
    }

    fn kick(&self, signal: Signal) {
        if let Some(kick) = &self.kick {
            let _ = kick.send(signal);
        }
    }

    pub fn spawn(mut self) -> EngineHandle {
        if let Some(worker) = self.attachments.take() {
            let (kick, inbox) = mpsc::channel();
            worker.spawn(self.notify.clone(), inbox);
            self.kick = Some(kick);
        }
        let (commands, inbox) = mpsc::channel();
        let push_commands = commands.clone();
        let thread = std::thread::Builder::new()
            .name("schlau-engine".into())
            .spawn(move || self.run(inbox, push_commands))
            .expect("engine thread starts");
        EngineHandle { commands, thread: Some(thread) }
    }

    fn run(mut self, inbox: Receiver<Command>, commands: Sender<Command>) {
        let Some(connect) = self.connect.clone() else {
            self.notify.set_status(Status::Offline);
            return self.run_local(inbox);
        };
        let mut remote: Option<Remote> = None;
        let mut pending = Pending {
            sync: true,
            backfill: true,
            background_bodies: true,
            load_model: self.semantic.is_some(),
            ..Default::default()
        };
        let mut backoff = Duration::from_secs(1);
        let mut push_started = false;
        let mut retry_in = Duration::ZERO;
        loop {
            let timeout = match (&remote, pending.has_work()) {
                (None, _) => retry_in,
                (Some(_), true) => Duration::ZERO,
                (Some(_), false) => IDLE_SYNC,
            };
            let first = inbox.recv_timeout(timeout);
            match first {
                Ok(Command::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(command) => {
                    self.handle(command, &mut pending);
                    while let Ok(command) = inbox.try_recv() {
                        if command == Command::Shutdown {
                            return;
                        }
                        self.handle(command, &mut pending);
                    }
                }
                Err(RecvTimeoutError::Timeout) if remote.is_some() && !pending.has_work() => pending.sync = true,
                Err(RecvTimeoutError::Timeout) => {}
            }

            if remote.is_none() {
                self.notify.set_status(Status::Connecting);
                match connect().map_err(Into::into).and_then(Remote::new) {
                    Ok(r) => match r.claim(&mut self.store) {
                        Ok(()) => {
                            remote = Some(r);
                            backoff = Duration::from_secs(1);
                            retry_in = Duration::ZERO;
                            if !push_started {
                                push::spawn(connect.clone(), commands.clone());
                                push_started = true;
                            }
                        }
                        Err(e) => {
                            self.fail(&e.to_string(), &mut backoff);
                            retry_in = backoff;
                        }
                    },
                    Err(e) => {
                        self.fail(&e.to_string(), &mut backoff);
                        retry_in = backoff;
                        continue;
                    }
                }
            }
            let Some(r) = remote.as_ref() else { continue };
            match self.step(r, &mut pending) {
                Ok(()) => backoff = Duration::from_secs(1),
                Err(e) => {
                    pending.sync = true;
                    self.fail(&e.to_string(), &mut backoff);
                }
            }
        }
    }

    fn fail(&self, error: &str, backoff: &mut Duration) {
        self.notify.set_status(Status::Error(error.to_string()));
        *backoff = (*backoff * 2).min(Duration::from_secs(120));
    }

    /// Does the most urgent piece of pending work: the open conversation's bodies, then the
    /// outbox and server changes, then older mail, then bodies in the background.
    fn step(&mut self, remote: &Remote, pending: &mut Pending) -> Result<(), crate::SyncError> {
        if !pending.bodies.is_empty() {
            let ids = std::mem::take(&mut pending.bodies);
            remote.fetch_bodies(&mut self.store, &ids)?;
            pending.embed = true;
            self.kick(Signal::Kick);
            self.notify.changed();
            return Ok(());
        }
        if pending.sync {
            pending.sync = false;
            remote.replay_outbox(&mut self.store)?;
            let mut changed = remote.sync_mailboxes(&mut self.store)?;
            changed |= remote.start_emails(&mut self.store)?;
            changed |= remote.sync_emails(&mut self.store)?;
            if changed {
                self.notify.changed();
            }
            self.status_idle_or_syncing(pending);
            return Ok(());
        }
        if pending.backfill {
            pending.backfill = remote.backfill(&mut self.store)?;
            self.notify.changed();
            self.status_idle_or_syncing(pending);
            return Ok(());
        }
        if pending.background_bodies {
            let fetched = remote.fetch_bodies(&mut self.store, &[])?;
            pending.background_bodies = !fetched.is_empty();
            if !fetched.is_empty() {
                self.kick(Signal::Kick);
            }
            pending.embed |= !fetched.is_empty();
            if !pending.background_bodies {
                self.store.optimize()?;
            }
            self.status_idle_or_syncing(pending);
            return Ok(());
        }
        self.local_step(pending);
        Ok(())
    }

    /// Work that needs no server: loading or downloading the model and embedding messages.
    fn local_step(&mut self, pending: &mut Pending) {
        let Some((shared, cache)) = self.semantic.clone() else {
            (pending.load_model, pending.download_model, pending.embed) = (false, false, false);
            return;
        };
        if pending.load_model {
            pending.load_model = false;
            pending.embed |= semantic::load_local(&cache, &shared);
            return;
        }
        if pending.download_model {
            pending.download_model = false;
            let notify = self.notify.clone();
            let result = semantic::download(&cache, &shared, |fraction| {
                notify.set_status(Status::Preparing(format!("Downloading the search model, {:.0}%", fraction * 100.0)));
            });
            match result {
                Ok(()) => pending.embed = true,
                Err(e) => self.notify.set_status(Status::Error(format!("model download failed: {e}"))),
            }
            return;
        }
        if pending.embed {
            let Some(embedder) = shared.get() else {
                pending.embed = false;
                return;
            };
            match semantic::embed_batch(&mut self.store, embedder.as_ref()) {
                Ok(0) => {
                    pending.embed = false;
                    self.notify.set_status(Status::Idle);
                    self.notify.changed();
                }
                Ok(_) => {
                    let total = self.store.reader().and_then(|r| r.vector_count()).unwrap_or(0);
                    self.notify.set_status(Status::Preparing(format!("Indexing meaning, {total} passages")));
                }
                Err(e) => {
                    pending.embed = false;
                    self.notify.set_status(Status::Error(e.to_string()));
                }
            }
        }
    }

    fn status_idle_or_syncing(&self, pending: &Pending) {
        let done = self.store.reader().and_then(|r| r.message_count()).unwrap_or(0) as u64;
        self.notify.set_status(if pending.backfill || pending.background_bodies {
            Status::Syncing { done, total: None }
        } else {
            Status::Idle
        });
    }

    fn handle(&mut self, command: Command, pending: &mut Pending) {
        match command {
            Command::Perform(action) => {
                match self.store.write(|tx| tx.perform(&action)) {
                    Ok(_) => self.notify.changed(),
                    Err(e) => self.notify.set_status(Status::Error(e.to_string())),
                }
                pending.sync = true;
            }
            Command::FetchBodies(ids) => pending.bodies.extend(ids),
            Command::Refresh => pending.sync = true,
            Command::EnableSemantic => {
                pending.download_model = self.semantic.as_ref().is_some_and(|(s, _)| s.get().is_none());
                pending.embed = true;
            }
            Command::EnableMediaSearch => self.kick(Signal::Enable),
            Command::Shutdown => {}
        }
    }

    fn run_local(mut self, inbox: Receiver<Command>) {
        let mut pending = Pending { load_model: self.semantic.is_some(), ..Default::default() };
        loop {
            let command = if pending.has_local_work() {
                inbox.try_recv().ok()
            } else {
                match inbox.recv() {
                    Ok(c) => Some(c),
                    Err(_) => return,
                }
            };
            match command {
                Some(Command::Shutdown) => return,
                Some(command) => self.handle(command, &mut pending),
                None => self.local_step(&mut pending),
            }
        }
    }
}

impl Pending {
    fn has_work(&self) -> bool {
        self.sync || !self.bodies.is_empty() || self.backfill || self.background_bodies || self.has_local_work()
    }

    fn has_local_work(&self) -> bool {
        self.load_model || self.download_model || self.embed
    }
}
