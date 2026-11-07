mod list;
mod overlay;
mod thread;

use rune::{Cx, El, col, row, spacer, text};
use rune_kit::Kit;
use schlau_sync::Status;

use crate::state::Ui;
use crate::theme;

pub use list::row_key;

pub fn view(s: &Ui, cx: &mut Cx<Ui>) -> El {
    let dark = cx.system_dark();
    let k = theme::kit(dark);
    let main = match (&s.thread, &s.web) {
        (Some(t), Some(id)) => thread::web_view(t, id, &k),
        (Some(t), None) => thread::thread_view(t, &k, dark),
        (None, _) => list::thread_list(s, &k, dark),
    };
    let base = col()
        .key("app")
        .w_full()
        .h_full()
        .fill(k.c.bg_base)
        .child(top_bar(s, &k))
        .child(main)
        .child(status_bar(s, &k));
    let overlays = [overlay::palette(s, &k, dark), overlay::search(s, &k, dark), toast(s, &k)];
    rune_kit::root(base, overlays.into_iter().flatten())
}

fn top_bar(s: &Ui, k: &Kit) -> El {
    let name = s.current_mailbox().map(|m| m.name.clone()).unwrap_or_else(|| "schlau".into());
    let unread = s.current_mailbox().map_or(0, |m| m.unread_threads.max(s.rows.iter().filter(|r| r.unread_count > 0).count() as i64));
    let count = if unread > 0 { format!("{unread} unread") } else { match s.rows.len() {
        1 => "1 conversation".to_string(),
        n => format!("{n} conversations"),
    } };
    row()
        .key("top-bar")
        .w_full()
        .h(52.0)
        .pl(86.0)
        .pr(18.0)
        .gap(12.0)
        .center_y()
        .window_drag()
        .child(text(name).font(rune::TextStyle { size: 15.0, weight: 650.0, ..k.ty.body }).color(k.c.text_base))
        .child(text(count).font(k.ty.small).color(k.c.text_faint))
        .child(spacer())
        .child(text("⌘K").font(k.ty.small).color(k.c.text_faint))
}

fn status_bar(s: &Ui, k: &Kit) -> El {
    let status = match &s.status {
        Status::Idle | Status::Offline if s.background.is_some() => s.background.clone().unwrap_or_default(),
        Status::Idle => "Up to date".to_string(),
        Status::Connecting => "Connecting…".to_string(),
        Status::Preparing(what) => what.clone(),
        Status::Offline => "Offline · local replica".to_string(),
        Status::Syncing { done, total: Some(total) } => format!("Syncing {done} of {total}"),
        Status::Syncing { done, total: None } => format!("Syncing {done}"),
        Status::Error(e) => format!("Sync error: {e}"),
    };
    let hints = if s.thread.is_some() {
        "j/k next · e done · # trash · s star · u back · ⌘K commands"
    } else {
        "j/k move · ↵ open · e done · # trash · / search · ⌘K commands"
    };
    let chord = s.pending_chord.map(|c| format!("{c} …")).unwrap_or_default();
    row()
        .key("status-bar")
        .w_full()
        .h(28.0)
        .px(18.0)
        .gap(16.0)
        .center_y()
        .fill(k.c.layer_01)
        .child(text(hints).font(k.ty.small).color(k.c.text_faint))
        .child(text(chord).font(k.ty.small).color(k.c.text_accent))
        .child(spacer())
        .child(text(status).font(k.ty.small).color(k.c.text_faint))
}

fn toast(s: &Ui, k: &Kit) -> Option<El> {
    let toast = s.toast.as_ref().filter(|t| t.until > s.clock)?;
    Some(
        col().key("toast-layer").absolute(0.0, 0.0).w_full().h_full().pb(44.0).justify_end().items_center().inert().child(
            row()
                .key("toast")
                .px(14.0)
                .h(34.0)
                .center_y()
                .radius(8.0)
                .fill(k.c.bg_inverse)
                .child(text(toast.text.clone()).font(k.ty.body).color(k.c.text_inverse)),
        ),
    )
}
