use rune::{Color, El, Key, KeyEvent, TextStyle, Tx, col, el, row, text, text_input};
use rune_kit::{Keybind, Kit};

use crate::commands::{self, Entry};
use crate::format::short_date;
use crate::rank;
use crate::state::Ui;
use crate::theme;

const PANEL_WIDTH: f32 = 640.0;
const MAX_ROWS: usize = 12;

fn panel(key: &str, k: &Kit, field: El, rows: Vec<El>, footer: El) -> El {
    let list = col().w_full().padding(6.0).gap(1.0).children(rows);
    let body = col()
        .key(key.to_string())
        .w_full()
        .max_w(PANEL_WIDTH)
        .radius(12.0)
        .clip_rounded(12.0)
        .fill(k.c.layer_01)
        .border(1.0, k.c.border_base)
        .child(row().w_full().h(52.0).px(18.0).center_y().child(field))
        .child(el().w_full().h(1.0).fill(k.c.border_muted))
        .child(list)
        .child(footer)
        .shadow(18.0, 48.0, Color::BLACK.alpha(0.35));
    col().absolute(0.0, 0.0).w_full().h_full().pt(110.0).px(16.0).items_center().fill(k.c.scrim.alpha(0.35)).child(body)
}

fn picker_row(key: String, picked: bool, k: &Kit, dark: bool, children: Vec<El>) -> El {
    row()
        .key(key)
        .w_full()
        .h(36.0)
        .px(12.0)
        .gap(10.0)
        .center_y()
        .radius(7.0)
        .fill(if picked { theme::selection(dark) } else { Color::TRANSPARENT })
        .hover(k.c.hover)
        .children(children)
}

/// Moves a picked index through `n` results on Up/Down (and Ctrl-N/P), wrapping.
fn step_picked(picked: &mut usize, n: usize, ev: &KeyEvent) -> bool {
    let down = matches!(ev.key, Key::Down) || (ev.mods.ctrl && matches!(&ev.key, Key::Text(t) if t == "n"));
    let up = matches!(ev.key, Key::Up) || (ev.mods.ctrl && matches!(&ev.key, Key::Text(t) if t == "p"));
    match (n, down, up) {
        (0, _, _) => false,
        (_, true, _) => {
            *picked = (*picked + 1) % n;
            true
        }
        (_, _, true) => {
            *picked = (*picked + n - 1) % n;
            true
        }
        _ => false,
    }
}

pub fn palette_results(s: &Ui) -> Vec<(Entry, String, String, Vec<usize>)> {
    let query = s.palette.as_ref().map(|p| p.query.text.as_str()).unwrap_or("");
    let entries = commands::palette_entries(s);
    let titles: Vec<&str> = entries.iter().map(|(_, t, _)| t.as_str()).collect();
    rank::fuzzy(query, &titles, MAX_ROWS)
        .into_iter()
        .map(|(i, positions)| {
            let (entry, title, keys) = entries[i].clone();
            (entry, title, keys, positions)
        })
        .collect()
}

pub fn palette(s: &Ui, k: &Kit, dark: bool) -> Option<El> {
    let p = s.palette.as_ref()?;
    let results = palette_results(s);
    let field = text_input(&p.query, TextStyle { size: 16.0, ..k.ty.body }.lh(24.0))
        .key("palette-query")
        .single_line()
        .grow(1.0)
        .color(k.c.text_base)
        .placeholder(if p.folders { "Go to folder" } else { "Type a command or folder" }, k.c.text_faint)
        .on_edit(|s: &mut Ui| &mut s.palette.as_mut().expect("palette open").query)
        .on_input_change(|s: &mut Ui, _| {
            if let Some(p) = s.palette.as_mut() {
                p.picked = 0;
            }
        })
        .on_input_key(|s: &mut Ui, ev: &KeyEvent, _tx: &mut Tx| {
            if matches!(ev.key, Key::Escape) {
                s.palette = None;
                return true;
            }
            let n = palette_results(s).len();
            s.palette.as_mut().is_some_and(|p| step_picked(&mut p.picked, n, ev))
        })
        .on_submit(|s: &mut Ui, tx: &mut Tx| {
            let picked = s.palette.as_ref().map_or(0, |p| p.picked);
            if let Some((entry, ..)) = palette_results(s).into_iter().nth(picked) {
                commands::run_entry(s, &entry, tx);
            }
        });
    let rows = results
        .into_iter()
        .enumerate()
        .map(|(i, (entry, title, keys, positions))| {
            let label = rank::highlighted(&title, &positions, k.ty.body, k.c.text_base, theme::accent(dark));
            let keycaps = if keys.is_empty() {
                el()
            } else if matches!(entry, Entry::Mailbox(_)) {
                text(format!("{keys} unread")).font(k.ty.small).color(k.c.text_faint)
            } else {
                Keybind::new(&keys.split([' ', '+']).map(keycap).collect::<Vec<_>>()).build(k)
            };
            let entry_for_press = entry.clone();
            picker_row(format!("p:{title}"), i == p.picked, k, dark, vec![label.grow(1.0), keycaps])
                .on_press_tx(move |s: &mut Ui, tx: &mut Tx| commands::run_entry(s, &entry_for_press, tx))
        })
        .collect();
    let footer = row()
        .w_full()
        .h(30.0)
        .px(18.0)
        .center_y()
        .child(text("↑↓ to pick · ↵ to run · esc to close").font(k.ty.small).color(k.c.text_faint));
    Some(panel("palette", k, field, rows, footer))
}

fn keycap(key: &str) -> String {
    match key {
        "mod" => "⌘".into(),
        "shift" => "⇧".into(),
        "alt" => "⌥".into(),
        "enter" => "↵".into(),
        "escape" => "esc".into(),
        other => other.to_string(),
    }
}

pub fn search(s: &Ui, k: &Kit, dark: bool) -> Option<El> {
    let search = s.search.as_ref()?;
    let field = text_input(&search.query, TextStyle { size: 16.0, ..k.ty.body }.lh(24.0))
        .key("search-query")
        .single_line()
        .grow(1.0)
        .color(k.c.text_base)
        .placeholder("Search mail: words, from:, is:unread, after:2026-01-01…", k.c.text_faint)
        .on_edit(|s: &mut Ui| &mut s.search.as_mut().expect("search open").query)
        .on_input_change(|s: &mut Ui, _| s.request_search())
        .on_input_key(|s: &mut Ui, ev: &KeyEvent, _tx: &mut Tx| {
            if matches!(ev.key, Key::Escape) {
                s.search = None;
                return true;
            }
            let Some(search) = s.search.as_mut() else { return false };
            let n = search.results.as_ref().map_or(0, |r| r.rows.len().min(MAX_ROWS));
            step_picked(&mut search.picked, n, ev)
        })
        .on_submit(|s: &mut Ui, _tx: &mut Tx| s.open_search_result());
    let now = crate::format::now_unix();
    let rows = search
        .results
        .iter()
        .flat_map(|r| r.rows.iter().zip(r.attachments.iter().chain(std::iter::repeat(&None))).take(MAX_ROWS))
        .enumerate()
        .map(|(i, (r, matched))| {
            let id = r.thread_id.clone();
            let matched = matched.as_ref().map(|name| {
                row()
                    .h(20.0)
                    .px(6.0)
                    .gap(4.0)
                    .center_y()
                    .radius(4.0)
                    .fill(k.c.layer_04)
                    .child(text("◆").font(k.ty.text_xsmall_medium).color(k.c.text_accent))
                    .child(text(name.clone()).font(k.ty.small).color(k.c.text_muted).truncate().max_w(200.0))
            });
            picker_row(
                format!("s:{}", r.thread_id),
                i == search.picked,
                k,
                dark,
                vec![
                    text(r.participants.clone()).font(TextStyle { weight: 600.0, ..k.ty.body }).color(k.c.text_base).truncate().w(150.0),
                    text(r.subject.clone()).font(k.ty.body).color(k.c.text_muted).truncate().grow(1.0).min_w(0.0),
                ]
                .into_iter()
                .chain(matched)
                .chain([text(short_date(r.latest_at, now)).font(k.ty.small).color(k.c.text_faint)])
                .collect(),
            )
            .on_press(move |s: &mut Ui| {
                s.search = None;
                s.open_thread(&id);
            })
        })
        .collect();
    let summary = match &search.results {
        Some(r) => format!("{} conversation{} · {:.1} ms", r.rows.len(), if r.rows.len() == 1 { "" } else { "s" }, r.elapsed_ms),
        None if s.search_results.is_pending() => "Searching…".into(),
        None => "Type to search".into(),
    };
    let footer = row().w_full().h(30.0).px(18.0).center_y().child(text(summary).font(k.ty.small).color(k.c.text_faint));
    Some(panel("search", k, field, rows, footer))
}
