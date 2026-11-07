use rune::{Color, El, Key, KeyEvent, ScrollSpec, TextStyle, Tx, col, row, text, text_input};
use rune_kit::Kit;

use crate::settings::{Choice, Section, Setting};
use crate::settings_pane::{self, PaneRow, ROWS, SettingsPane};
use crate::state::Ui;
use crate::theme;

const SECTIONS: [Section; 3] = [Section::Words, Section::Meaning, Section::Preview];

fn section_of(row: PaneRow) -> Section {
    match row {
        PaneRow::Setting(setting) => setting.section(),
        PaneRow::ServerUrl | PaneRow::ServerModel => Section::Meaning,
    }
}

pub fn row_key(index: usize) -> String {
    format!("setting:{index}")
}

pub fn settings_view(s: &Ui, pane: &SettingsPane, k: &Kit, dark: bool) -> El {
    let title = TextStyle { size: 20.0, weight: 600.0, ..k.ty.body }.lh(28.0);
    let header = col()
        .w_full()
        .pb(6.0)
        .gap(4.0)
        .child(text("Settings").font(title).color(k.c.text_base))
        .child(text("Changes apply at once and are saved to settings.json.").font(k.ty.small).color(k.c.text_faint));
    let sections = SECTIONS.into_iter().map(|section| {
        let rows: Vec<El> = ROWS
            .iter()
            .enumerate()
            .filter(|(_, r)| section_of(**r) == section)
            .map(|(i, &r)| pane_row(s, pane, i, r, k, dark))
            .collect();
        col()
            .w_full()
            .gap(8.0)
            .child(text(section.title().to_uppercase()).font(k.ty.text_xsmall_medium).color(k.c.text_faint).pl(4.0))
            .child(
                col()
                    .w_full()
                    .padding(4.0)
                    .gap(2.0)
                    .radius(10.0)
                    .fill(k.c.layer_01)
                    .border(1.0, k.c.border_muted)
                    .children(rows),
            )
    });
    let page = col().w_full().max_w(820.0).px(28.0).py(24.0).gap(22.0).child(header).children(sections);
    col()
        .key("settings")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(row().w_full().justify_center().child(page))
}

fn pane_row(s: &Ui, pane: &SettingsPane, index: usize, r: PaneRow, k: &Kit, dark: bool) -> El {
    let picked = index == pane.picked;
    let body = match r {
        PaneRow::Setting(setting) => setting_row(s, setting, k, dark),
        PaneRow::ServerUrl => field_row(s, pane, r, k),
        PaneRow::ServerModel => field_row(s, pane, r, k),
    };
    col()
        .key(row_key(index))
        .w_full()
        .px(14.0)
        .py(12.0)
        .radius(7.0)
        .fill(if picked { theme::selection(dark) } else { Color::TRANSPARENT })
        .child(body)
        .on_press(move |s: &mut Ui| settings_pane::pick(s, r))
}

fn setting_row(s: &Ui, setting: Setting, k: &Kit, dark: bool) -> El {
    let settings = s.settings.effective();
    let current = setting.current(&settings);
    let chips = setting.choices().into_iter().map(|choice| chip(choice, choice == current, k, dark));
    let forced = setting == Setting::AttachmentMeaning && s.settings.env_server().is_some();
    let detail = match forced {
        true => format!("{} Set by SCHLAU_EMBED_URL for this run.", settings_pane::detail(s, current)),
        false => settings_pane::detail(s, current),
    };
    col()
        .w_full()
        .gap(6.0)
        .child(
            row()
                .w_full()
                .gap(12.0)
                .center_y()
                .child(text(setting.name()).font(TextStyle { weight: 600.0, ..k.ty.body }).color(k.c.text_base).grow(1.0))
                .child(row().gap(4.0).children(chips)),
        )
        .child(text(detail).font(k.ty.small).color(k.c.text_muted).wrap().w_full())
}

fn chip(choice: Choice, on: bool, k: &Kit, dark: bool) -> El {
    let accent = theme::accent(dark);
    row()
        .key(format!("choice:{choice:?}"))
        .h(28.0)
        .px(10.0)
        .center_y()
        .radius(6.0)
        .fill(if on { accent.alpha(0.22) } else { Color::TRANSPARENT })
        .border(1.0, if on { accent.alpha(0.6) } else { k.c.border_muted })
        .hover(k.c.hover)
        .child(text(choice.label()).font(k.ty.small).color(if on { k.c.text_base } else { k.c.text_muted }))
        .on_press(move |s: &mut Ui| {
            settings_pane::pick(s, PaneRow::Setting(choice.setting()));
            if !choice.is_current(&s.settings.effective()) {
                s.choose(choice);
            }
        })
}

fn field_row(s: &Ui, pane: &SettingsPane, r: PaneRow, k: &Kit) -> El {
    let (name, edit, placeholder) = match r {
        PaneRow::ServerUrl => ("Embeddings server", &pane.url, "http://127.0.0.1:8080"),
        _ => ("Server model", &pane.model, "embeddinggemma-2"),
    };
    let field = text_input(edit, k.ty.body)
        .key(match r {
            PaneRow::ServerUrl => "settings-url",
            _ => "settings-model",
        })
        .single_line()
        .grow(1.0)
        .color(k.c.text_base)
        .placeholder(placeholder, k.c.text_faint)
        .on_edit(move |s: &mut Ui| {
            let pane = s.settings_pane.as_mut().expect("settings open");
            match r {
                PaneRow::ServerUrl => &mut pane.url,
                _ => &mut pane.model,
            }
        })
        .on_input_key(move |s: &mut Ui, ev: &KeyEvent, tx: &mut Tx| {
            if !matches!(ev.key, Key::Escape | Key::Tab) {
                return false;
            }
            settings_pane::save_server(s, tx);
            true
        })
        .on_submit(|s: &mut Ui, tx: &mut Tx| settings_pane::save_server(s, tx));
    let editing = edit.focused;
    let box_ = row()
        .w(360.0)
        .h(32.0)
        .px(10.0)
        .center_y()
        .radius(6.0)
        .fill(k.c.layer_02)
        .border(1.0, if editing { k.c.border_strong } else { k.c.border_muted })
        .child(field);
    let detail = match (r, editing, s.settings.env_server()) {
        (_, true, _) => Some("↵ saves.".to_string()),
        (PaneRow::ServerUrl, _, Some(env)) => Some(format!("SCHLAU_EMBED_URL sets {} for this run.", env.url)),
        (PaneRow::ServerUrl, _, None) => {
            Some("llama-server or any OpenAI-compatible /v1/embeddings endpoint. Used by the Server choices.".to_string())
        }
        _ => None,
    };
    col()
        .w_full()
        .gap(6.0)
        .child(
            row()
                .w_full()
                .gap(12.0)
                .center_y()
                .child(text(name).font(TextStyle { weight: 600.0, ..k.ty.body }).color(k.c.text_base).grow(1.0))
                .child(box_),
        )
        .children(detail.map(|d| text(d).font(k.ty.small).color(k.c.text_muted).wrap().w_full()))
}
