//! The screener as a list, one sender per row with its decision keys in reach, and the setup page
//! that walks through the Fastmail rules the screener relies on.

use rune::{Color, El, ScrollSpec, TextStyle, col, el, row, spacer, text};
use rune_kit::{Kit, StateColors};
use schlau_store::{ContactGroup, ScreenerSender};
use schlau_sync::{DestinationKind, ScreenerConfig};

use crate::format::short_date;
use crate::screener::{self, SetupRow};
use crate::state::Ui;
use crate::theme;

fn sized(base: TextStyle, size: f32, weight: f32) -> TextStyle {
    TextStyle { size, weight, line_height: 0.0, ..base }
}

fn keycap(key: &str, k: &Kit) -> El {
    row()
        .h(18.0)
        .px(5.0)
        .center()
        .radius(4.0)
        .fill(k.c.layer_04)
        .border(1.0, k.c.border_muted)
        .child(text(key.to_string()).font(sized(k.ty.mono, 11.0, 500.0)).color(k.c.text_base))
}

fn chip(label: impl Into<String>, fill: Color, ink: Color, k: &Kit) -> El {
    row().h(20.0).px(8.0).center_y().radius(10.0).fill(fill).child(text(label.into()).font(sized(k.ty.body, 11.0, 500.0)).color(ink))
}

fn state_chip(label: impl Into<String>, colors: StateColors, k: &Kit) -> El {
    chip(label, colors.bg, colors.fg, k)
}

/// The destination name a key stands for.
fn destination_label(s: &Ui, config: &ScreenerConfig, index: usize) -> String {
    match config.destinations.get(index).map(|d| d.kind) {
        Some(DestinationKind::Inbox) => "Inbox".into(),
        Some(DestinationKind::Out) => "Spam".into(),
        Some(DestinationKind::File) => screener::mailbox_name(s, &config.destinations[index].mailbox_id),
        None => String::new(),
    }
}

fn legend(s: &Ui, config: &ScreenerConfig, k: &Kit) -> El {
    let order = screener::destination_of(config, DestinationKind::Inbox)
        .into_iter()
        .chain(screener::file_destinations(config))
        .chain(screener::destination_of(config, DestinationKind::Out));
    let items = order.map(|i| {
        row()
            .gap(6.0)
            .center_y()
            .child(keycap(&screener::destination_key(config, i), k))
            .child(text(destination_label(s, config, i)).font(k.ty.small).color(k.c.text_muted))
    });
    row()
        .w_full()
        .px(32.0)
        .pb(10.0)
        .gap(16.0)
        .center_y()
        .children(items)
        .child(spacer())
        .child(row().gap(6.0).center_y().child(keycap("x", k)).child(text("mark").font(k.ty.small).color(k.c.text_faint)))
        .child(row().gap(6.0).center_y().child(keycap("⇧A", k)).child(text("accept suggestions").font(k.ty.small).color(k.c.text_faint)))
}

fn section_heading(title: &str, count: usize, k: &Kit) -> El {
    row()
        .w_full()
        .px(32.0)
        .pt(14.0)
        .pb(6.0)
        .gap(10.0)
        .center_y()
        .child(text(title.to_uppercase()).font(k.ty.text_xsmall_medium).color(k.c.text_faint))
        .child(text(count.to_string()).font(k.ty.text_xsmall_medium).color(k.c.text_faint))
}

#[allow(clippy::too_many_arguments)]
fn sender_row(
    s: &Ui,
    config: &ScreenerConfig,
    groups: &[ContactGroup],
    sender: &ScreenerSender,
    index: usize,
    selected: bool,
    marked: bool,
    k: &Kit,
    dark: bool,
) -> El {
    let accent = theme::accent(dark);
    let now = crate::format::now_unix();
    let name = if sender.name.trim().is_empty() { sender.email.clone() } else { sender.name.clone() };
    let unread = sender.unread > 0;
    let mark = el()
        .size(12.0, 12.0)
        .radius(3.0)
        .fill(if marked { accent } else { Color::TRANSPARENT })
        .border(1.0, if marked { accent } else { k.c.text_faint });
    let title = row()
        .w_full()
        .gap(8.0)
        .center_y()
        .child(text(name).font(sized(k.ty.body, 14.5, if unread { 650.0 } else { 560.0 })).color(k.c.text_base).truncate().min_w(0.0))
        .child(text(sender.message_ids.len().to_string()).font(sized(k.ty.body, 11.0, 500.0)).color(k.c.text_faint));
    let line = format!("{}  ·  {}", sender.latest_subject, sender.email);
    let subtitle = text(line).font(sized(k.ty.body, 12.5, 400.0)).color(k.c.text_muted).truncate().min_w(0.0);
    let suggested = screener::suggestion(config, groups, sender).map(|i| {
        let key = screener::destination_key(config, i);
        chip(format!("→ {} {key}", destination_label(s, config, i)), accent.alpha(0.18), k.c.text_base, k)
    });
    let facts = [
        sender.replied.then(|| chip("you replied", k.c.layer_03, k.c.text_muted, k)),
        sender.contact.then(|| chip("contact", k.c.layer_03, k.c.text_muted, k)),
        (!sender.elsewhere.is_empty()).then(|| {
            let n: i64 = sender.elsewhere.iter().map(|(_, n)| n).sum();
            chip(format!("{n} elsewhere"), k.c.layer_03, k.c.text_muted, k)
        }),
    ];
    let keys = selected.then(|| {
        let order = screener::destination_of(config, DestinationKind::Inbox)
            .into_iter()
            .chain(screener::file_destinations(config).into_iter().take(4))
            .chain(screener::destination_of(config, DestinationKind::Out));
        row().gap(4.0).center_y().children(order.map(|i| keycap(&screener::destination_key(config, i), k)))
    });
    let email = sender.email.clone();
    row()
        .key(screener::row_key(&sender.email))
        .w_full()
        .h(56.0)
        .pl(32.0)
        .pr(32.0)
        .gap(14.0)
        .center_y()
        .fill(if selected { theme::selection(dark) } else { Color::TRANSPARENT })
        .hover(k.c.hover)
        .child(mark)
        .child(col().grow(1.0).min_w(0.0).gap(3.0).child(title).child(subtitle))
        .child(row().gap(6.0).center_y().children(facts.into_iter().flatten()).children(suggested).children(keys))
        .child(text(short_date(sender.latest_at, now)).font(sized(k.ty.body, 12.0, 400.0)).color(k.c.text_faint).w(56.0))
        .on_press(move |s: &mut Ui| {
            if let Some(screener) = s.screener.as_mut()
                && let Some(i) = screener.senders.iter().position(|x| x.email == email)
            {
                screener.selected = i;
            }
        })
        .on_double_press(move |s: &mut Ui| {
            if let Some(thread) = s.screener.as_ref().and_then(|x| x.senders.get(index)).map(|x| x.latest_thread.clone()) {
                s.open_thread(&thread);
            }
        })
}

pub fn screener_view(s: &Ui, k: &Kit, dark: bool) -> El {
    let Some(screener) = s.screener.as_ref() else { return el() };
    let config = screener::config(s);
    let groups = s.reader.contact_groups().unwrap_or_default();
    if screener.senders.is_empty() {
        let folder = config.triage.as_deref().map(|t| screener::mailbox_name(s, t)).unwrap_or_default();
        return col()
            .grow(1.0)
            .w_full()
            .center()
            .gap(6.0)
            .child(text("Nobody waiting.").font(k.ty.body).color(k.c.text_muted))
            .child(text(format!("New senders land in {folder}. g s comes back here.")).font(k.ty.small).color(k.c.text_faint));
    }
    let suggested: Vec<bool> = screener.senders.iter().map(|x| screener::suggestion(&config, &groups, x).is_some()).collect();
    let rows = |want: bool| -> Vec<El> {
        screener
            .senders
            .iter()
            .enumerate()
            .filter(|(i, _)| suggested[*i] == want)
            .map(|(i, x)| sender_row(s, &config, &groups, x, i, i == screener.selected, screener.marked.contains(&x.email), k, dark))
            .collect()
    };
    let (with, without) = (rows(true), rows(false));
    let sections = [("Suggested", with), ("New senders", without)]
        .into_iter()
        .filter(|(_, rows)| !rows.is_empty())
        .flat_map(|(title, rows)| std::iter::once(section_heading(title, rows.len(), k)).chain(rows));
    col()
        .key("screener")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(legend(s, &config, k))
        .child(el().w_full().h(1.0).fill(k.c.border_weak))
        .child(col().key("screener-rows").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(col().w_full().pb(24.0).children(sections)))
}

pub fn setup_row_key(index: usize) -> String {
    format!("setup:{index}")
}

/// A row's title, the detail under it, and its status chip.
fn setup_row_parts(s: &Ui, config: &ScreenerConfig, groups: &[ContactGroup], row: SetupRow, k: &Kit) -> (String, Vec<String>, Option<El>) {
    let contacts = screener::contacts_available(s);
    match row {
        SetupRow::Contacts => match contacts {
            Some(true) => ("Contacts access".into(), vec!["The API token can read and change contacts.".into()], Some(state_chip("ready", k.c.success, k))),
            Some(false) => (
                "Contacts access".into(),
                vec![
                    "The API token can't use contacts, where the screener keeps its decisions.".into(),
                    "In Fastmail: Settings → Privacy & Security → API tokens → New token, with Contacts allowed.".into(),
                    "Replace the token in the Keychain item “schlau” (or FASTMAIL_API_TOKEN) and restart.".into(),
                ],
                Some(state_chip("missing", k.c.warning, k)),
            ),
            None => ("Contacts access".into(), vec!["Checked on the next sync.".into()], Some(state_chip("waiting", k.c.info, k))),
        },
        SetupRow::Triage => match &config.triage {
            Some(id) => (
                format!("New senders land in {}", screener::mailbox_name(s, id)),
                vec!["↵ picks another folder.".into()],
                Some(state_chip("ready", k.c.success, k)),
            ),
            None => (
                "Folder for new senders".into(),
                vec![format!("↵ uses your “{0}” folder, or makes one called {0}.", screener::TRIAGE_NAME)],
                Some(state_chip("to do", k.c.warning, k)),
            ),
        },
        SetupRow::Destination(i) => {
            let d = &config.destinations[i];
            let key = screener::destination_key(config, i);
            let what = match d.kind {
                DestinationKind::Inbox => "Let in: Inbox, marked read".to_string(),
                DestinationKind::Out => format!("Send to hell: {}, as spam", screener::mailbox_name(s, &d.mailbox_id)),
                DestinationKind::File => format!("File in {}", screener::mailbox_name(s, &d.mailbox_id)),
            };
            let group = groups.iter().find(|g| g.name == d.group);
            let status = match (group, contacts) {
                (Some(g), _) => state_chip(format!("{} in group", g.members), k.c.success, k),
                (None, Some(true)) => state_chip("making group…", k.c.info, k),
                (None, _) => state_chip("needs contacts", k.c.warning, k),
            };
            let remove = match d.kind {
                DestinationKind::File => " · d removes it",
                _ => "",
            };
            (format!("{key}   {what}"), vec![format!("Contact group “{}”{remove}", d.group)], Some(status))
        }
        SetupRow::AddDestination => ("Add a folder…".into(), vec!["↵ picks a folder to file senders in, with the next digit.".into()], None),
        SetupRow::Rule(i) => {
            let d = &config.destinations[i];
            let folder = screener::mailbox_name(s, &d.mailbox_id);
            let stats = groups.iter().find(|g| g.name == d.group).and_then(|g| s.reader.routing_stats(&g.id).ok()).unwrap_or_default();
            let status = match (stats.routed, stats.missed) {
                (_, m) if m > 0 => state_chip(format!("missed {m}: check it"), k.c.warning, k),
                (r, _) if r > 0 => state_chip(format!("working · {r} routed"), k.c.success, k),
                _ => state_chip("not seen yet", k.c.info, k),
            };
            (
                format!("Rule: {} → {folder}", d.group),
                vec![
                    format!("Name: schlau · {folder}"),
                    format!("If: the sender is a member of the contact group “{}”", d.group),
                    format!("Then: move to “{folder}”, and stop applying other rules"),
                ],
                Some(status),
            )
        }
        SetupRow::CatchAll => {
            let folder = config.triage.as_deref().map(|t| screener::mailbox_name(s, t)).unwrap_or_else(|| screener::TRIAGE_NAME.into());
            (
                format!("Rule: everyone else → {folder}"),
                vec![
                    "Name: schlau · screen new senders".into(),
                    "If: From → Sender is not a contact".into(),
                    format!("Then: move to “{folder}”. Put this rule below the others."),
                ],
                None,
            )
        }
    }
}

fn section_of(row: SetupRow) -> usize {
    match row {
        SetupRow::Contacts => 0,
        SetupRow::Triage | SetupRow::Destination(_) | SetupRow::AddDestination => 1,
        SetupRow::Rule(_) | SetupRow::CatchAll => 2,
    }
}

const SECTIONS: [(&str, &str); 3] = [
    ("Fastmail access", "Decisions are kept in Fastmail contact groups, so they hold on every device."),
    ("Folders", "Where new senders wait, and the places a decision sends them. Each has its contact group."),
    (
        "Rules in Fastmail",
        "Fastmail lets only its website make rules. Add these once in Settings → Filters & Rules (o opens it, c copies a group's name). schlau checks each by watching where new mail lands, and moves what a missing rule leaves behind while it runs.",
    ),
];

pub fn setup_view(s: &Ui, k: &Kit, dark: bool) -> El {
    let Some(setup) = s.screener_setup.as_ref() else { return el() };
    let config = screener::config(s);
    let groups = s.reader.contact_groups().unwrap_or_default();
    let rows = screener::setup_rows(&config);
    let picked = setup.picked.min(rows.len().saturating_sub(1));
    let title = TextStyle { size: 20.0, weight: 600.0, ..k.ty.body }.lh(28.0);
    let header = col()
        .w_full()
        .pb(6.0)
        .gap(4.0)
        .child(text("Screener setup").font(title).color(k.c.text_base))
        .child(
            text("New mail from strangers waits in one folder until you let the sender in, file them, or send them to spam. g s opens the screener.")
                .font(k.ty.small)
                .color(k.c.text_faint)
                .wrap()
                .w_full(),
        );
    let sections = SECTIONS.iter().enumerate().map(|(n, (name, about))| {
        let items: Vec<El> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| section_of(**r) == n)
            .map(|(i, &r)| {
                let (head, lines, status) = setup_row_parts(s, &config, &groups, r, k);
                let body = col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(4.0)
                    .child(text(head).font(TextStyle { weight: 600.0, ..k.ty.body }).color(k.c.text_base))
                    .children(lines.into_iter().map(|l| text(l).font(k.ty.small).color(k.c.text_muted).wrap().w_full()));
                row()
                    .key(setup_row_key(i))
                    .w_full()
                    .px(14.0)
                    .py(12.0)
                    .gap(12.0)
                    .radius(7.0)
                    .fill(if i == picked { theme::selection(dark) } else { Color::TRANSPARENT })
                    .child(body)
                    .children(status)
                    .on_press(move |s: &mut Ui| {
                        if let Some(setup) = s.screener_setup.as_mut() {
                            setup.picked = i;
                        }
                    })
            })
            .collect();
        col()
            .w_full()
            .gap(8.0)
            .child(text(name.to_uppercase()).font(k.ty.text_xsmall_medium).color(k.c.text_faint).pl(4.0))
            .child(text(*about).font(k.ty.small).color(k.c.text_faint).wrap().w_full().pl(4.0))
            .child(col().w_full().padding(4.0).gap(2.0).radius(10.0).fill(k.c.layer_01).border(1.0, k.c.border_muted).children(items))
    });
    let page = col().w_full().max_w(820.0).px(28.0).py(24.0).gap(22.0).child(header).children(sections);
    col()
        .key("screener-setup")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .child(row().w_full().justify_center().child(page))
}
