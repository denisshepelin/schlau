use rune::{El, ScrollSpec, TextStyle, col, row, text};
use rune_kit::Kit;
use schlau_store::{Address, MessageView};

use crate::format::long_date;
use crate::reader::{self, Rendered};
use crate::state::{OpenThread, Ui};
use crate::theme;

/// The focused message in the secure web view: a header, and a slot the native web view covers.
pub fn web_view(t: &OpenThread, message_id: &str, k: &Kit) -> El {
    let message = t.messages.iter().find(|m| m.id == message_id);
    let document = t.bodies.get(message_id).and_then(|r| r.document.clone()).unwrap_or_default();
    let title = message.map(|m| m.subject.clone()).unwrap_or_default();
    let note = t.bodies.get(message_id).and_then(|r| reader::privacy_note(&r.report)).unwrap_or_else(|| "No trackers found".into());
    col()
        .key("web")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(
            row()
                .w_full()
                .h(40.0)
                .px(20.0)
                .gap(12.0)
                .center_y()
                .child(text(title).font(TextStyle { weight: 600.0, ..k.ty.body }).color(k.c.text_base).truncate().min_w(0.0).grow(1.0))
                .child(text(format!("{note} · JavaScript off · remote content blocked · v to close")).font(k.ty.small).color(k.c.text_faint)),
        )
        .child(
            col()
                .key("webview-slot")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .fill(rune::Color::WHITE)
                .on_drawn_layout(move |_: &mut Ui, rect| crate::webview::show(&document, rect)),
        )
}

pub fn thread_view(t: &OpenThread, k: &Kit, dark: bool) -> El {
    let subject = t.messages.first().map(|m| m.subject.as_str()).unwrap_or("(no subject)");
    let title = TextStyle { size: 20.0, weight: 600.0, ..k.ty.body }.lh(28.0);
    let header = col()
        .w_full()
        .pb(8.0)
        .gap(4.0)
        .child(text(subject.to_string()).font(title).color(k.c.text_base).selectable(true).selection_color(k.selection()).wrap().w_full())
        .child(
            text(format!("{} message{}", t.messages.len(), if t.messages.len() == 1 { "" } else { "s" }))
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    let messages = t.messages.iter().enumerate().map(|(i, m)| {
        let expanded = t.expanded.get(i).copied().unwrap_or(true);
        message_card(m, t.bodies.get(&m.id), expanded, i == t.focused, k, dark)
    });
    let page = col().w_full().max_w(820.0).px(28.0).py(24.0).gap(10.0).child(header).children(messages);
    col()
        .key("thread")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: false })
        .on_scroll_offset({
            let at = t.scroll.clone();
            move |pos| at.set(pos)
        })
        .child(row().w_full().justify_center().child(page))
}

fn sender(m: &MessageView) -> Option<&Address> {
    m.addresses.from.first()
}

fn message_card(m: &MessageView, body: Option<&Rendered>, expanded: bool, focused: bool, k: &Kit, dark: bool) -> El {
    let name = sender(m).map(|a| a.display().to_string()).unwrap_or_else(|| "(unknown sender)".into());
    let email = sender(m).map(|a| a.email.clone()).unwrap_or_default();
    let id = m.id.clone();
    let head = row()
        .w_full()
        .gap(8.0)
        .center_y()
        .child(text(name).font(TextStyle { weight: 600.0, ..k.ty.body }).color(k.c.text_base))
        .child(text(email).font(k.ty.small).color(k.c.text_faint).truncate().min_w(0.0).grow(1.0))
        .child(text(long_date(m.received_at)).font(k.ty.small).color(k.c.text_faint))
        .on_press(move |s: &mut Ui| s.toggle_message(&id));
    let card = col()
        .key(format!("m:{}", m.id))
        .w_full()
        .padding(16.0)
        .gap(10.0)
        .radius(10.0)
        .fill(k.c.layer_01)
        .border(1.0, if focused { theme::accent(dark).alpha(0.5) } else { k.c.border_muted })
        .child(head);
    if !expanded {
        return card.child(text(m.preview.clone()).font(k.ty.body).color(k.c.text_muted).truncate());
    }
    let recipients = m
        .addresses
        .to
        .iter()
        .chain(&m.addresses.cc)
        .map(|a| a.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let card = card.child(text(format!("to {recipients}")).font(k.ty.small).color(k.c.text_faint).truncate());
    match body {
        Some(body) => card.child(reader::body_view(body, k, theme::accent(dark))),
        None => card.child(
            col()
                .gap(6.0)
                .child(text(m.preview.clone()).font(k.ty.body.lh(21.0)).color(k.c.text_base).selectable(true).selection_color(k.selection()).wrap_anywhere().w_full())
                .child(text("Loading the full message…").font(k.ty.small).color(k.c.text_faint)),
        ),
    }
}
