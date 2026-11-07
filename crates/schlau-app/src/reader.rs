//! The native reader: draws a message body as blocks the app controls, so nothing in it can load,
//! run or track. Bodies are sanitized once, when they arrive, and kept with the open conversation.

use rune::{Color, El, Link, Span, TextStyle, Tx, col, el, rich, row, text};
use rune_kit::Kit;
use schlau_html::{Block, ImageSource, Inline, Options, TrackerReport};
use schlau_store::{Attachment, Body};

use crate::state::Ui;

/// A body ready to draw: reader blocks, what sanitizing removed, and the HTML for the web view.
#[derive(Debug, Clone, PartialEq)]
pub struct Rendered {
    pub blocks: Vec<Block>,
    pub report: TrackerReport,
    pub document: Option<String>,
    /// Files to show under the message: everything but inline images the body already places.
    pub attachments: Vec<Attachment>,
}

pub fn render(body: &Body) -> Rendered {
    let rendered = render_content(body);
    Rendered { attachments: body.attachments.iter().filter(|a| !(a.inline && a.cid.is_some())).cloned().collect(), ..rendered }
}

fn render_content(body: &Body) -> Rendered {
    let options = Options::default();
    match (&body.html, &body.text) {
        (Some(html), _) => {
            let sanitized = schlau_html::sanitize(html, &options);
            Rendered {
                blocks: schlau_html::to_blocks_with(html, &options),
                document: Some(schlau_html::wrap_document(&sanitized, &Default::default())),
                report: sanitized.report,
                attachments: Vec::new(),
            }
        }
        (None, Some(text)) => {
            let sanitized = schlau_html::text_to_html(text);
            Rendered {
                blocks: schlau_html::to_blocks(&sanitized.html),
                document: None,
                report: sanitized.report,
                attachments: Vec::new(),
            }
        }
        (None, None) => {
            Rendered { blocks: Vec::new(), report: TrackerReport::default(), document: None, attachments: Vec::new() }
        }
    }
}

/// One line about what the sanitizer kept from loading, or nothing when the message was clean.
pub fn privacy_note(report: &TrackerReport) -> Option<String> {
    let mut parts = Vec::new();
    if report.pixels_removed > 0 {
        parts.push(format!("{} tracking pixel{}", report.pixels_removed, plural(report.pixels_removed)));
    }
    if report.remote_blocked > 0 {
        parts.push(format!("{} remote image{}", report.remote_blocked, plural(report.remote_blocked)));
    }
    if report.links_unwrapped + report.params_stripped > 0 {
        parts.push(format!("{} tracked link{}", report.links_unwrapped + report.params_stripped, plural(report.links_unwrapped + report.params_stripped)));
    }
    (!parts.is_empty()).then(|| {
        let who = report.tracker_domains.iter().take(2).cloned().collect::<Vec<_>>().join(", ");
        let who = if who.is_empty() { String::new() } else { format!(" ({who})") };
        format!("Blocked {}{who}", parts.join(", "))
    })
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

pub fn body_view(rendered: &Rendered, k: &Kit, accent: Color) -> El {
    let note = privacy_note(&rendered.report).map(|n| {
        row()
            .gap(6.0)
            .center_y()
            .child(text("◐").font(k.ty.small).color(accent))
            .child(text(n).font(k.ty.small).color(k.c.text_faint))
    });
    col()
        .w_full()
        .min_w(0.0)
        .gap(12.0)
        .selection_group(true)
        .children(note)
        .children(rendered.blocks.iter().map(|b| block(b, k, accent, 0)))
        .children((!rendered.attachments.is_empty()).then(|| attachments(&rendered.attachments, k)))
}

fn kind(a: &Attachment) -> &'static str {
    match a.mime.split('/').next().unwrap_or("") {
        "image" => "Image",
        "audio" => "Audio",
        "video" => "Video",
        _ if a.mime == "application/pdf" => "PDF",
        _ if a.mime == "text/calendar" => "Invite",
        _ => "File",
    }
}

pub fn human_size(bytes: i64) -> String {
    match bytes {
        b if b >= 1_000_000 => format!("{:.1} MB", b as f64 / 1_000_000.0),
        b if b >= 1_000 => format!("{} KB", b / 1_000),
        b => format!("{b} B"),
    }
}

fn attachments(list: &[Attachment], k: &Kit) -> El {
    row().w_full().gap(8.0).pt(4.0).flex_wrap().children(list.iter().map(|a| {
        let name = a.name.clone().unwrap_or_else(|| "attachment".into());
        row()
            .key(format!("a:{}", a.blob_id))
            .h(34.0)
            .px(10.0)
            .gap(8.0)
            .center_y()
            .radius(8.0)
            .fill(k.c.layer_02)
            .border(1.0, k.c.border_muted)
            .child(
                row()
                    .h(18.0)
                    .px(5.0)
                    .center()
                    .radius(4.0)
                    .fill(k.c.layer_04)
                    .child(text(kind(a)).font(k.ty.text_xsmall_medium).color(k.c.text_muted)),
            )
            .child(text(name).font(k.ty.small).color(k.c.text_base).truncate().max_w(260.0))
            .child(text(human_size(a.size)).font(k.ty.small).color(k.c.text_faint))
    }))
}

fn paragraph(inlines: &[Inline], style: TextStyle, k: &Kit, accent: Color) -> El {
    let mut content = String::new();
    let mut spans = Vec::new();
    let mut links = Vec::new();
    for inline in inlines {
        let start = content.len();
        match inline {
            Inline::Text { text, bold, italic, code } => {
                content.push_str(text);
                let span = Span::new(start, content.len());
                let span = if *bold { span.bold() } else { span };
                let span = if *italic { span.italic() } else { span };
                spans.push(if *code { span.mono() } else { span });
            }
            Inline::Link { text, href } => {
                content.push_str(text);
                spans.push(Span::new(start, content.len()).color(accent));
                links.push(Link::new(start..content.len(), href.clone()));
            }
            Inline::LineBreak => content.push('\n'),
        }
    }
    let el = rich(content, spans).font(style).color(k.c.text_base).selectable(true).selection_color(k.selection()).wrap_anywhere().w_full();
    links
        .into_iter()
        .fold(el, El::link)
        .on_link(|_: &mut Ui, url: &str, tx: &mut Tx| {
            let _ = tx.open_url(url);
        })
}

fn block(b: &Block, k: &Kit, accent: Color, depth: usize) -> El {
    let body = k.ty.body.lh(21.0);
    match b {
        Block::Paragraph(inlines) => paragraph(inlines, body, k, accent),
        Block::Heading(level, inlines) => {
            let size = match level {
                1 => 19.0,
                2 => 17.0,
                _ => 15.0,
            };
            paragraph(inlines, TextStyle { size, weight: 650.0, ..body }.lh(size * 1.35), k, accent)
        }
        Block::Quote(blocks) if depth < 8 => row()
            .w_full()
            .gap(12.0)
            .child(el().w(2.0).h_full().radius(1.0).fill(k.c.border_base))
            .child(col().grow(1.0).min_w(0.0).gap(8.0).opacity(0.8).children(blocks.iter().map(|b| block(b, k, accent, depth + 1)))),
        Block::Quote(_) => el(),
        Block::List { ordered, items } => col().w_full().gap(4.0).children(items.iter().enumerate().map(|(i, item)| {
            let marker = if *ordered { format!("{}.", i + 1) } else { "•".into() };
            row()
                .w_full()
                .gap(8.0)
                .child(text(marker).font(body).color(k.c.text_faint).w(18.0))
                .child(col().grow(1.0).min_w(0.0).gap(4.0).children(item.iter().map(|b| block(b, k, accent, depth + 1))))
        })),
        Block::Image { alt, src } => {
            let label = match src {
                ImageSource::Blocked(_) | ImageSource::Remote(_) => "remote image, not loaded",
                _ => "image",
            };
            let alt = if alt.trim().is_empty() { label.to_string() } else { format!("{alt} · {label}") };
            row()
                .px(10.0)
                .py(6.0)
                .radius(6.0)
                .fill(k.c.layer_02)
                .child(text(alt).font(k.ty.small).color(k.c.text_faint).truncate())
        }
        Block::Rule => el().w_full().h(1.0).fill(k.c.border_muted),
        Block::Pre(code) => col()
            .w_full()
            .padding(10.0)
            .radius(6.0)
            .fill(k.c.layer_02)
            .child(text(code.clone()).font(k.ty.mono).color(k.c.text_base).selectable(true).selection_color(k.selection()).wrap_anywhere().w_full()),
    }
}
