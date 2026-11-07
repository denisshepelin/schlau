mod account;
mod commands;
mod demo;
mod format;
mod preview;
mod quicklook;
mod rank;
mod reader;
mod remote_images;
mod search;
mod settings;
mod settings_pane;
mod state;
mod theme;
mod view;
mod webview;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rune::{App, AppSpec, KeyEvent, Tx, Waker};
use schlau_store::Store;
use schlau_sync::{AttachmentWorker, BlobSource, Connect, Engine, FixtureBlobs, JmapBlobs, Notify, Search, SharedMedia};

use crate::state::Ui;

struct Args {
    db: Option<PathBuf>,
    demo: bool,
    demo_size: usize,
    fixtures: Option<PathBuf>,
    /// Index attachments before the window opens, so a headless snapshot sees image search results.
    index_attachments: bool,
}

impl Args {
    /// Runs without an account: synthetic demo data or a fixture mailbox.
    fn offline(&self) -> bool {
        self.demo || self.fixtures.is_some()
    }
}

fn parse_args(args: &[String]) -> Args {
    let value = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    Args {
        db: value("--db").map(PathBuf::from),
        demo: args.iter().any(|a| a == "--demo"),
        demo_size: value("--demo-size").and_then(|v| v.parse().ok()).unwrap_or(20_000),
        fixtures: value("--fixtures").map(PathBuf::from),
        index_attachments: args.iter().any(|a| a == "--index-attachments"),
    }
}

fn data_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    let dir = if cfg!(target_os = "macos") {
        home.join("Library/Application Support/schlau")
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("schlau")
    };
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn models_dir() -> PathBuf {
    data_dir().join("models")
}

fn open_store(args: &Args) -> (PathBuf, Store) {
    let name = match (&args.fixtures, args.demo) {
        (Some(_), _) => "fixtures.sqlite",
        (None, true) => "demo.sqlite",
        (None, false) => "mail.sqlite",
    };
    let path = args.db.clone().unwrap_or_else(|| data_dir().join(name));
    let mut store = Store::open(&path).unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display()));
    if let Some(dir) = &args.fixtures {
        schlau_sync::load_fixtures(&mut store, dir, format::now_unix())
            .unwrap_or_else(|e| panic!("cannot load fixtures from {}: {e}", dir.display()));
    } else if args.demo {
        demo::seed(&mut store, args.demo_size, format::now_unix()).expect("demo data");
    }
    (path, store)
}

/// Where attachment files come from: the account, or the files next to a fixture mailbox. The demo
/// data has none.
fn blob_source(args: &Args, connect: Option<Connect>) -> Option<Arc<dyn BlobSource>> {
    match (&args.fixtures, connect) {
        (Some(dir), _) => Some(Arc::new(FixtureBlobs(dir.join("blobs")))),
        (None, Some(connect)) => Some(Arc::new(JmapBlobs::new(connect))),
        (None, None) => None,
    }
}

fn attachment_worker(
    args: &Args,
    path: &Path,
    connect: Option<Connect>,
    shared: &SharedMedia,
    settings: &settings::Settings,
) -> Option<AttachmentWorker> {
    let blobs = blob_source(args, connect)?;
    Some(AttachmentWorker { path: path.to_path_buf(), blobs, backend: settings.search.attachment_backend(&models_dir()), shared: shared.clone() })
}

fn setup(s: &mut Ui, waker: Waker) {
    s.search_results.set_waker(waker.clone());
    s.preview_loads.set_waker(waker.clone());
    s.preview_files.set_waker(waker.clone());
    s.images = Some(rune::ImageCache::new(waker.clone()));
    s.remote_images.set_waker(waker.clone());
    let Some(store) = s.pending_store.take() else { return };
    let notify = Notify::new(move || waker());
    let search = Search {
        settings: s.settings.effective().search,
        cache: s.models_dir.clone(),
        text: s.embedder.clone(),
        media: s.media.clone(),
    };
    let engine = Engine::new(store, notify.clone()).with_search(search);
    let engine = match s.pending_connect.take() {
        Some(connect) => engine.with_remote(connect),
        None => engine,
    };
    let engine = match s.pending_worker.take() {
        Some(worker) => engine.with_attachments(worker),
        None => engine,
    };
    s.engine = Some(engine.spawn());
    s.notify = Some(notify);
    search::warm(&s.db_path, 0, None);
}

fn poll(s: &mut Ui, clock: f64, tx: &mut Tx) -> bool {
    s.clock = clock;
    let mut changed = false;
    if let Some(images) = s.images.as_ref()
        && s.remote_images.poll(images)
    {
        changed = true;
    }
    if settings_pane::focus_pending(s) {
        changed = true;
    }
    if let Some(key) = s.pending_scroll.take() {
        tx.scroll_into_view(key);
        changed = true;
    }
    if let Some(notify) = &s.notify {
        let version = notify.version();
        let status = notify.status();
        let background = notify.background();
        if version != s.seen_version {
            s.seen_version = version;
            s.reload();
            changed = true;
        }
        if status != s.status {
            s.status = status;
            changed = true;
        }
        if background != s.background {
            s.background = background;
            changed = true;
        }
    }
    let models = (s.embedder.get().map(|m| m.id().to_string()), s.attachment_model().map(|m| m.id().to_string()));
    if models != s.models_in_use {
        s.models_in_use = models;
        if s.search.is_some() {
            s.request_search();
        }
    }
    if let Some(files) = s.preview_files.take() {
        s.preview_files_loaded(files);
        changed = true;
    }
    if let Some(loaded) = s.preview_loads.take() {
        s.preview_loaded(loaded);
        changed = true;
    }
    if let Some(results) = s.search_results.take()
        && let Some(search) = s.search.as_mut()
    {
        let current = results.query == search.query.text;
        search.results = Some(results);
        if current && search.open_when_ready {
            s.open_search_result();
        }
        changed = true;
    }
    if let Some(toast) = &s.toast {
        if toast.until <= clock {
            s.toast = None;
            changed = true;
        } else {
            tx.wake_after(Duration::from_secs_f64(toast.until - clock));
        }
    }
    changed
}

fn on_key(s: &mut Ui, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if s.palette.is_some() || s.search.is_some() {
        return false;
    }
    if s.preview.is_some() {
        return preview::on_key(s, ev);
    }
    if quicklook::is_open() && preview::on_system_key(s, ev) {
        return true;
    }
    if s.settings_pane.is_some() {
        return settings_pane::on_key(s, ev, tx);
    }
    let before = (s.selected, s.thread.as_ref().map(|t| t.thread_id.clone()));
    let handled = commands::dispatch(s, ev, tx);
    let after = (s.selected, s.thread.as_ref().map(|t| t.thread_id.clone()));
    if handled && before != after && s.thread.is_none()
        && let Some(row) = s.selected_row()
    {
        tx.scroll_into_view(view::row_key(&row.thread_id));
    }
    handled
}

fn main() {
    rune::prewarm_gpu();
    let args = parse_args(rune::extra_args());
    let (path, store) = open_store(&args);
    let spec = AppSpec {
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        min_size: Some((720.0, 480.0)),
        ..AppSpec::new("schlau", (1180.0, 780.0), || unreachable!("state comes from init"), view::view)
    };
    let connect = (!args.offline()).then(account::Account::from_environment).flatten().map(account::Account::connect);
    let media = SharedMedia::default();
    let settings_file = settings::SettingsFile::load(&path.with_file_name("settings.json"), &models_dir());
    let settings = settings_file.effective();
    if args.index_attachments
        && let Some(worker) = attachment_worker(&args, &path, connect.clone(), &media, &settings)
        && let Err(e) = worker.run_until_idle()
    {
        eprintln!("indexing attachments failed: {e}");
    }
    let worker = attachment_worker(&args, &path, connect.clone(), &media, &settings);
    let blobs = blob_source(&args, connect.clone());
    let cache = data_dir().join("attachments");
    App::new(spec)
        .init(move || {
            let mut ui = Ui::new(path, store, connect);
            ui.media = media;
            ui.pending_worker = worker;
            ui.blobs = blobs;
            ui.attachment_cache = cache;
            ui.settings = settings_file;
            ui.models_dir = models_dir();
            ui
        })
        .on_window(|window| {
            let _ = window.with_raw_handle(|handle| {
                webview::attach(handle.as_raw());
                quicklook::attach(handle.as_raw());
            });
        })
        .run();
}
