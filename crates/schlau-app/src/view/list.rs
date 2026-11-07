//! The conversation list as a page of headlines: the rows grouped by period under serif headings,
//! each row the sender's avatar, the subject in large type, and the sender and snippet beneath.

use std::borrow::Cow;

use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use rune::{Color, El, GrowthSpec, Icon, ScrollSpec, Span, TextStyle, col, el, icon, rich, row, spacer, text};
use rune_kit::Kit;
use schlau_store::ThreadRow;

use crate::periods::{Section, section_at};
use crate::state::Ui;
use crate::theme;

/// What a row is estimated at before it is measured; a period's first row is taller by its heading.
pub const ROW_ESTIMATE: f32 = 62.0;
const ROW_HEIGHT: f32 = 56.0;

const PAPERCLIP: Icon = Icon {
    name: "schlau-paperclip",
    view: 16.0,
    body: r#"<path d="M10.5 4.5 5.6 9.4a1.6 1.6 0 0 0 2.3 2.3l5-5a3 3 0 0 0-4.3-4.3L3.4 7.6a4.3 4.3 0 0 0 6.1 6.1L13 10.2" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

pub fn row_key(thread_id: &str) -> String {
    format!("t:{thread_id}")
}

pub fn today(now: i64) -> Date {
    Timestamp::from_second(now).map(|t| t.to_zoned(TimeZone::system()).date()).unwrap_or_default()
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
            ROW_ESTIMATE,
            |s: &Ui, i| Cow::Owned(row_key(&s.rows[i].thread_id)),
            move |s: &Ui, i| {
                let k = theme::kit(dark);
                let sections = s.sections();
                let at = section_at(&sections, i);
                entry(s, i, sections.get(at).filter(|sec| sec.start == i).map(|sec| (sec, at == 0)), &k, dark)
            },
        )
        .virtual_rows_version((s.mailbox_id.as_str(), s.rows_version))
        .on_visible_rows(|s: &mut Ui, range| range.end + 80 > s.rows.len() && s.load_more());
    col().key("list").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(rows)
}

/// Row `i`, under its period's heading when it opens the period.
fn entry(s: &Ui, i: usize, opens: Option<(&Section, bool)>, k: &Kit, dark: bool) -> El {
    let r = &s.rows[i];
    let now = crate::format::now_unix();
    let tz = TimeZone::system();
    let sections = s.sections();
    let period = sections[section_at(&sections, i)].period;
    let time = period.time_of(r.latest_at, &tz);
    col()
        .key(row_key(&r.thread_id))
        .w_full()
        .children(opens.map(|(section, first)| period_heading(section, first, today(now), k, dark)))
        .child(thread_row(r, i == s.selected, &time, &s.own_names, k, dark))
}

fn sized(base: TextStyle, size: f32, weight: f32) -> TextStyle {
    TextStyle { size, weight, line_height: 0.0, ..base }
}

fn period_heading(section: &Section, first: bool, today: Date, k: &Kit, dark: bool) -> El {
    let small = sized(k.ty.body, 12.0, 400.0);
    let count = match section.len() {
        1 => "1 conversation".to_string(),
        n => format!("{n} conversations"),
    };
    let tally = match section.unread {
        0 => text(count).font(small).color(k.c.text_faint),
        n => {
            let line = format!("{count} · {n} unread");
            rich(line.clone(), vec![Span::new(count.len(), line.len()).color(theme::accent(dark)).weight(600.0)])
                .font(small)
                .color(k.c.text_faint)
        }
    };
    col()
        .w_full()
        .pt(if first { 10.0 } else { 22.0 })
        .child(
            row()
                .w_full()
                .px(32.0)
                .pb(9.0)
                .gap(14.0)
                .items_end()
                .child(text(section.period.title(today)).font(theme::period_heading(k.ty.body)).color(k.c.text_base))
                .child(text(section.period.dates(section.first_day)).font(small).color(k.c.text_faint).pb(3.0))
                .child(spacer())
                .child(tally.pb(3.0)),
        )
        .child(el().w_full().h(1.0).fill(k.c.border_weak))
        .child(el().h(4.0))
}

/// The names to draw avatars for: the participants other than the account itself, at most two,
/// and how many more there are.
fn faces<'a>(participants: &'a str, own: &[String]) -> (Vec<&'a str>, usize) {
    let names: Vec<&str> = participants.split(", ").filter(|n| *n != "…" && !n.is_empty()).collect();
    let mine = |n: &&str| own.iter().any(|o| o == n || o.split_whitespace().next() == Some(n));
    let others: Vec<&str> = names.iter().copied().filter(|n| !mine(n)).collect();
    let shown = if others.is_empty() { names.iter().copied().take(1).collect() } else { others.iter().copied().take(2).collect() };
    (shown, others.len().saturating_sub(2))
}

/// Up to two letters for an avatar: the initials of the first two words that start with a letter.
fn initials(name: &str) -> String {
    let letters: String = name
        .split_whitespace()
        .filter_map(|w| w.chars().next().filter(|c| c.is_alphabetic()))
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    if letters.is_empty() { name.chars().take(1).collect() } else { letters }
}

fn avatar(name: &str, size: f32, k: &Kit, dark: bool) -> El {
    let (fill, ink) = theme::avatar(name, dark);
    col()
        .size(size, size)
        .radius(size / 2.0)
        .fill(fill)
        .center()
        .child(text(initials(name)).font(sized(k.ty.body, (size * 0.36).round(), 650.0)).color(ink))
}

fn avatars(names: &[&str], more: usize, k: &Kit, dark: bool) -> El {
    let frame = el().w(50.0).h(36.0);
    match names {
        [one] => frame.child(avatar(one, 34.0, k, dark).absolute(0.0, 1.0)),
        [first, second, ..] => frame
            .child(avatar(second, 24.0, k, dark).border(2.0, k.c.bg_base).absolute(19.0, 6.0).size(26.0, 26.0))
            .child(avatar(first, 24.0, k, dark).border(2.0, k.c.bg_base).absolute(0.0, 6.0).size(26.0, 26.0))
            .children((more > 0).then(|| text(format!("+{more}")).font(sized(k.ty.body, 10.0, 500.0)).color(k.c.text_faint).absolute(2.0, 33.0))),
        [] => frame,
    }
}

fn thread_row(r: &ThreadRow, selected: bool, time: &str, own: &[String], k: &Kit, dark: bool) -> El {
    let unread = r.unread_count > 0;
    let (strong, soft) = if unread { (k.c.text_base, k.c.text_base) } else { (k.c.text_muted, k.c.text_muted) };
    let subject = text(r.subject.clone())
        .font(TextStyle { tracking: -0.1, ..sized(k.ty.body, 15.5, if unread { 650.0 } else { 500.0 }) })
        .color(strong)
        .truncate()
        .min_w(0.0);

    let count = if r.message_count > 1 { format!(" {}", r.message_count) } else { String::new() };
    let head = format!("{}{count}", r.participants);
    let line = format!("{head}  {}", r.preview);
    let meta = rich(
        line.clone(),
        vec![
            Span::new(0, r.participants.len()).weight(560.0).color(soft),
            Span::new(r.participants.len(), head.len()).size(11.0).color(k.c.text_faint),
            Span::new(head.len(), line.len()).color(k.c.text_faint),
        ],
    )
    .font(sized(k.ty.body, 12.5, 400.0))
    .truncate()
    .min_w(0.0);

    let marks = row()
        .gap(7.0)
        .center_y()
        .children(r.flagged.then(|| text("★").font(sized(k.ty.body, 12.0, 400.0)).color(theme::accent(dark))))
        .children(r.has_attachment.then(|| icon(&PAPERCLIP, 13.0).color(k.c.text_faint)));
    let side = col()
        .items_end()
        .gap(5.0)
        .pl(24.0)
        .child(text(time.to_string()).font(TextStyle { features: "'tnum' 1", ..sized(k.ty.body, 12.0, 400.0) }).color(k.c.text_faint))
        .child(marks);

    let (names, more) = faces(&r.participants, own);
    let dot = el().size(6.0, 6.0).radius(3.0).fill(if unread { theme::accent(dark) } else { Color::TRANSPARENT });
    let id = r.thread_id.clone();
    let open = r.thread_id.clone();
    row()
        .w_full()
        .h(ROW_HEIGHT)
        .pl(14.0)
        .pr(32.0)
        .gap(12.0)
        .center_y()
        .fill(if selected { theme::selection(dark) } else { Color::TRANSPARENT })
        .hover(k.c.hover)
        .child(dot)
        .child(avatars(&names, more, k, dark))
        .child(col().grow(1.0).min_w(0.0).gap(2.0).child(subject).child(meta))
        .child(side)
        .on_press(move |s: &mut Ui| s.select_thread(&id))
        .on_double_press(move |s: &mut Ui| s.open_thread(&open))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatars_skip_the_account_and_show_at_most_two() {
        let own = vec!["Denis Shepelin".to_string()];
        assert_eq!(faces("Mom, Denis", &own), (vec!["Mom"], 0));
        assert_eq!(faces("…, Priya, Denis, Tomás", &own), (vec!["Priya", "Tomás"], 0));
        assert_eq!(faces("Jane, Marcus, Priya", &own), (vec!["Jane", "Marcus"], 1));
        assert_eq!(faces("Denis Shepelin", &own), (vec!["Denis Shepelin"], 0), "a thread of one's own keeps its sender");
        assert_eq!(faces("", &own), (vec![], 0));
    }

    #[test]
    fn initials_take_two_words_that_start_with_letters() {
        assert_eq!(initials("Sam Okafor"), "SO");
        assert_eq!(initials("sqlite-users digest"), "SD");
        assert_eq!(initials("Tracklane"), "T");
        assert_eq!(initials("Hausverwaltung Krüger GmbH"), "HK");
        assert_eq!(initials("株式会社"), "株");
        assert_eq!(initials("123"), "1");
    }
}
