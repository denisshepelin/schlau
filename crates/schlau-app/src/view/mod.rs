mod list;
mod overlay;
mod settings;
mod thread;

use rune::{Color, Cx, El, TextStyle, col, row, spacer, text};
use rune_kit::Kit;
use schlau_sync::Status;

use crate::state::Ui;
use crate::theme;

pub use list::row_key;
pub use settings::row_key as settings_row_key;

pub fn view(s: &Ui, cx: &mut Cx<Ui>) -> El {
    let dark = cx.system_dark();
    let k = theme::kit(dark);
    let main = match (&s.settings_pane, &s.thread, &s.web) {
        (Some(pane), ..) => settings::settings_view(s, pane, &k, dark),
        (None, thread, web) => main_view(s, thread, web, &k, dark),
    };

    let base = col()
        .key("app")
        .w_full()
        .h_full()
        .fill(k.c.bg_base)
        .child(top_bar(s, &k, dark))
        .child(main)
        .child(status_bar(s, &k));
    let overlays = [crate::preview::view(s, &k), overlay::palette(s, &k, dark), overlay::search(s, &k, dark), toast(s, &k)];
    rune_kit::root(base, overlays.into_iter().flatten())
}

fn main_view(s: &Ui, thread: &Option<crate::state::OpenThread>, web: &Option<String>, k: &Kit, dark: bool) -> El {
    match (thread, web) {
        (Some(t), Some(id)) => thread::web_view(t, id, k),
        (Some(t), None) => thread::thread_view(t, &s.remote_images, k, dark),
        (None, _) => list::thread_list(s, k, dark),
    }
}

/// The list's periods as chips with their unread counts, the selected conversation's lit: a map
/// of the mailbox that `]d` and `[d` move through. A click jumps to the period.
fn period_strip(s: &Ui, k: &Kit, dark: bool) -> El {
    const SHOWN: usize = 10;
    let sections = s.sections();
    let at = crate::periods::section_at(&sections, s.selected);
    let first = at.saturating_sub(SHOWN / 2).min(sections.len().saturating_sub(SHOWN));
    let today = list::today(crate::format::now_unix());
    let label = TextStyle { size: 12.5, line_height: 0.0, ..k.ty.body };
    let chips = sections.iter().enumerate().skip(first).take(SHOWN).map(|(i, section)| {
        let current = i == at;
        let ink = match i.cmp(&at) {
            std::cmp::Ordering::Equal => k.c.text_base,
            std::cmp::Ordering::Less => k.c.text_faint.alpha(0.7),
            std::cmp::Ordering::Greater => k.c.text_faint,
        };
        let start = section.start;
        row()
            .key(format!("period-{i}"))
            .h(28.0)
            .px(11.0)
            .gap(6.0)
            .center_y()
            .radius(14.0)
            .fill(if current { k.c.layer_01 } else { Color::TRANSPARENT })
            .border(if current { 1.0 } else { 0.0 }, k.c.border_weak)
            .hover(k.c.hover)
            .child(text(section.period.chip(today)).font(TextStyle { weight: if current { 600.0 } else { 400.0 }, ..label }).color(ink))
            .children((section.unread > 0).then(|| {
                let accent = if i < at { k.c.text_faint } else { theme::accent(dark) };
                text(section.unread.to_string()).font(TextStyle { size: 11.0, weight: 650.0, line_height: 0.0, ..k.ty.body }).color(accent)
            }))
            .on_press_tx(move |s: &mut Ui, tx| {
                if let Some(key) = s.select_row(start) {
                    crate::commands::reveal_at_top(s, key, tx);
                }
            })
    });
    row().key("periods").grow(1.0).min_w(0.0).gap(2.0).center_y().clip_fade(0.0, 0.0, 24.0, 0.0).children(chips)
}

fn top_bar(s: &Ui, k: &Kit, dark: bool) -> El {
    let name = s.current_mailbox().map(|m| m.name.clone()).unwrap_or_else(|| "schlau".into());
    let title = text(name).font(rune::TextStyle { size: 15.0, weight: 650.0, ..k.ty.body }).color(k.c.text_base);
    if s.thread.is_none() && s.settings_pane.is_none() && !s.rows.is_empty() {
        return row()
            .key("top-bar")
            .w_full()
            .h(52.0)
            .pl(86.0)
            .pr(18.0)
            .gap(20.0)
            .center_y()
            .window_drag()
            .child(title)
            .child(period_strip(s, k, dark))
            .child(text("⌘K").font(k.ty.small).color(k.c.text_faint));
    }
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
        .child(title)
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
    let hints = if s.settings_pane.is_some() {
        "j/k move · h/l change · ↵ edit · esc close"
    } else if s.thread.is_some() {
        "j/k scroll · ⇧J/⇧K next · n/p message · e done · u back · ⌘K commands"
    } else {
        "j/k move · ]d/[d next/previous period · ↵ open · e done · / search · ⌘K commands"
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
