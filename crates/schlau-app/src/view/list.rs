use std::borrow::Cow;

use rune::{Color, El, GrowthSpec, ScrollSpec, Span, TextStyle, col, el, rich, row, text};
use rune_kit::Kit;
use schlau_store::ThreadRow;

use crate::format::short_date;
use crate::state::Ui;
use crate::theme;

pub const ROW_HEIGHT: f32 = 40.0;

pub fn row_key(thread_id: &str) -> String {
    format!("t:{thread_id}")
}

pub fn thread_list(s: &Ui, k: &Kit, dark: bool) -> El {
    if s.rows.is_empty() {
        return col().grow(1.0).w_full().center().child(
            text(if s.has_account || !s.mailboxes.is_empty() {
                "Nothing here. Inbox zero."
            } else {
                "No account yet. Set FASTMAIL_TOKEN (a Fastmail API token) and restart, or run with --demo."
            })
            .font(k.ty.body)
            .color(k.c.text_faint),
        );
    }
    let rows = col()
        .key("rows")
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            s.rows.len(),
            ROW_HEIGHT,
            |s: &Ui, i| Cow::Owned(row_key(&s.rows[i].thread_id)),
            move |s: &Ui, i| {
                let k = theme::kit(dark);
                thread_row(&s.rows[i], i == s.selected, crate::format::now_unix(), &k, dark)
            },
        )
        .virtual_rows_version((s.mailbox_id.as_str(), s.rows_version))
        .on_visible_rows(|s: &mut Ui, range| range.end + 80 > s.rows.len() && s.load_more());
    col().key("list").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(rows)
}

fn thread_row(r: &ThreadRow, selected: bool, now: i64, k: &Kit, dark: bool) -> El {
    let unread = r.unread_count > 0;
    let weight = if unread { 650.0 } else { 400.0 };
    let ink = if unread { k.c.text_base } else { k.c.text_muted };
    let sender = TextStyle { weight, ..k.ty.body };

    let subject_len = r.subject.len();
    let line = format!("{}   {}", r.subject, r.preview);
    let summary = rich(
        line.clone(),
        vec![
            Span::new(0, subject_len).weight(weight).color(ink),
            Span::new(subject_len, line.len()).color(k.c.text_faint),
        ],
    )
    .font(k.ty.body)
    .truncate()
    .grow(1.0)
    .min_w(0.0);

    let dot = el().size(6.0, 6.0).radius(3.0).fill(if unread { theme::accent(dark) } else { Color::TRANSPARENT });
    let count = if r.message_count > 1 {
        text(r.message_count.to_string()).font(k.ty.small).color(k.c.text_faint)
    } else {
        el()
    };
    let marks = row()
        .gap(6.0)
        .center_y()
        .child(if r.flagged { text("★").font(k.ty.small).color(theme::accent(dark)) } else { el() })
        .child(if r.has_attachment { text("⎙").font(k.ty.small).color(k.c.text_faint) } else { el() });

    let id = r.thread_id.clone();
    let open = r.thread_id.clone();
    row()
        .key(row_key(&r.thread_id))
        .w_full()
        .h(ROW_HEIGHT)
        .px(18.0)
        .gap(12.0)
        .center_y()
        .fill(if selected { theme::selection(dark) } else { Color::TRANSPARENT })
        .hover(k.c.hover)
        .child(dot)
        .child(
            row()
                .w(190.0)
                .gap(6.0)
                .center_y()
                .child(text(r.participants.clone()).font(sender).color(ink).truncate().min_w(0.0))
                .child(count),
        )
        .child(summary)
        .child(marks)
        .child(text(short_date(r.latest_at, now)).font(k.ty.small).color(k.c.text_faint))
        .on_press(move |s: &mut Ui| s.select_thread(&id))
        .on_double_press(move |s: &mut Ui| s.open_thread(&open))
}
