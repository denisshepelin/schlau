mod common;

use common::{fixture, receipt_cids};
use schlau_html::{
    Block, BodyKind, ColorScheme, ImageSource, Inline, Options, RemotePolicy, TextFormat, Theme,
    content_security_policy, sanitize, snippet, text_to_html, text_to_html_with, to_blocks,
    to_blocks_with, to_plain_text, wrap_document,
};

fn inline_text(inlines: &[Inline]) -> String {
    inlines
        .iter()
        .map(|inline| match inline {
            Inline::Text { text, .. } | Inline::Link { text, .. } => text.as_str(),
            Inline::LineBreak => "\n",
        })
        .collect()
}

fn all_blocks(blocks: &[Block]) -> Vec<&Block> {
    blocks
        .iter()
        .flat_map(|block| {
            let nested = match block {
                Block::Quote(inner) => all_blocks(inner),
                Block::List { items, .. } => {
                    items.iter().flat_map(|item| all_blocks(item)).collect()
                }
                _ => Vec::new(),
            };
            std::iter::once(block).chain(nested)
        })
        .collect()
}

fn links(blocks: &[Block]) -> Vec<(String, String)> {
    all_blocks(blocks)
        .into_iter()
        .flat_map(|block| match block {
            Block::Paragraph(inlines) | Block::Heading(_, inlines) => inlines.clone(),
            _ => Vec::new(),
        })
        .filter_map(|inline| match inline {
            Inline::Link { text, href } => Some((text, href)),
            _ => None,
        })
        .collect()
}

#[test]
fn csp_is_strict_and_tracks_remote_policy() {
    let blocked = content_security_policy(&Options::default());
    assert!(blocked.starts_with("default-src 'none'"));
    assert!(blocked.contains("script-src 'none'"));
    assert!(blocked.contains("style-src 'unsafe-inline'"));
    assert!(blocked.contains("img-src data: schlau-cid:;"));
    assert!(blocked.contains("font-src data:"));
    assert!(!blocked.contains("https:"));
    let allowed = content_security_policy(&Options::with_remote(RemotePolicy::Allow));
    assert!(allowed.contains("img-src data: schlau-cid: https:"));
    assert!(!allowed.contains("script-src 'unsafe"));
}

#[test]
fn wrapped_document_puts_csp_first_and_loads_nothing() {
    let sanitized = sanitize(&fixture("mailchimp_newsletter.html"), &Options::default());
    let document = wrap_document(&sanitized, &Theme::default());
    assert!(document.starts_with("<!DOCTYPE html>"));
    let csp_at = document.find("Content-Security-Policy").expect("csp meta");
    let style_at = document.find("<style>").expect("base style");
    assert!(csp_at < style_at);
    assert!(document.contains("img-src data: schlau-cid:;"));
    assert!(document.contains("name=\"referrer\" content=\"no-referrer\""));
    assert!(document.contains("max-width:100%"));
    let head = &document[..document.find("</head>").expect("head")];
    assert!(!head.contains("://"));
}

#[test]
fn plain_text_document_is_dark_mode_aware() {
    let sanitized = text_to_html("Hello");
    assert_eq!(sanitized.kind, BodyKind::PlainText);
    let auto = wrap_document(&sanitized, &Theme::default());
    assert!(auto.contains("prefers-color-scheme:dark"));
    let dark = wrap_document(
        &sanitized,
        &Theme {
            color_scheme: ColorScheme::Dark,
            ..Theme::default()
        },
    );
    assert!(dark.contains("color-scheme\" content=\"dark\""));
    let html = wrap_document(
        &sanitize("<p>x</p>", &Options::default()),
        &Theme::default(),
    );
    assert!(!html.contains("prefers-color-scheme"));
}

#[test]
fn text_to_html_escapes_linkifies_and_quotes() {
    let plain = "Hi <team> & friends,\r\nsee https://example.com/report?utm_source=mail&id=4.\r\nWrite to ops@example.org\r\n\r\nOn Tue, Bo wrote:\r\n> first level\r\n>> second level https://www.google.com/url?q=https://inner.example/x\r\n> back to first\r\n-- \r\nBo\r\nhttps://bo.example";
    let out = text_to_html(plain);
    let html = &out.html;
    assert!(html.contains("Hi &lt;team&gt; &amp; friends,"));
    assert!(html.contains("<a href=\"https://example.com/report?id=4\""));
    assert!(html.contains(">https://example.com/report?utm_source=mail&amp;id=4</a>."));
    assert!(html.contains("href=\"mailto:ops@example.org\""));
    assert!(html.contains("<blockquote class=\"schlau-quote\">first level<blockquote class=\"schlau-quote\">second level"));
    assert!(html.contains("href=\"https://inner.example/x\""));
    assert!(html.contains("</blockquote>back to first</blockquote>"));
    assert!(
        html.contains("<div class=\"schlau-signature\">-- \nBo\n<a href=\"https://bo.example/\"")
    );
    assert_eq!(out.report.params_stripped, 1);
    assert_eq!(out.report.links_unwrapped, 1);
    assert!(!html.contains("<script"));
}

#[test]
fn text_to_html_handles_format_flowed() {
    let plain = "This paragraph was \r\nwrapped by the \r\nsender.\r\n> quoted and \r\n> flowed\r\n From space-stuffed line";
    let out = text_to_html_with(
        plain,
        TextFormat {
            flowed: true,
            delsp: false,
        },
    );
    assert!(
        out.html
            .contains("This paragraph was wrapped by the sender.")
    );
    assert!(
        out.html
            .contains("<blockquote class=\"schlau-quote\">quoted and flowed</blockquote>")
    );
    assert!(out.html.contains("From space-stuffed line"));
    let delsp = text_to_html_with(
        "Zusammen \r\ngeschrieben",
        TextFormat {
            flowed: true,
            delsp: true,
        },
    );
    assert!(delsp.html.contains("Zusammengeschrieben"));
}

#[test]
fn plain_text_of_newsletter_is_readable() {
    let text = to_plain_text(&fixture("mailchimp_newsletter.html"), false);
    assert!(text.contains("Ethiopian naturals are back"));
    assert!(text.contains("Our Guji Hambela lot landed this week."));
    assert!(text.contains("- Free shipping over $40"));
    assert!(!text.contains("p{margin"));
    assert!(
        !text.contains("The Weekly Roast - Issue 142"),
        "title must be skipped"
    );
    assert!(
        !text.contains("brewing guide"),
        "hidden preheader must be skipped"
    );
    assert!(!text.contains('\u{200c}'));
    assert!(!text.contains("  "));
    assert!(!text.contains("\n\n\n"));
}

#[test]
fn plain_text_strips_gmail_quote() {
    let html = fixture("gmail_reply.html");
    let full = to_plain_text(&html, false);
    assert!(full.contains("Could we move the review"));
    let stripped = to_plain_text(&html, true);
    assert!(stripped.starts_with("Sounds good, Thursday at 3pm works for me."));
    assert!(stripped.ends_with("Priya"));
    assert!(!stripped.contains("Jonas"));
    assert!(!stripped.contains("wrote:"));
}

#[test]
fn plain_text_strips_outlook_history() {
    let html = fixture("outlook_safelinks.html");
    let full = to_plain_text(&html, false);
    assert!(full.contains("Can you share the Q4 numbers"));
    let stripped = to_plain_text(&html, true);
    assert!(stripped.contains("The Q4 budget is ready for review: Q4-budget.xlsx"));
    assert!(stripped.ends_with("Alex"));
    assert!(!stripped.contains("Morgan"));
    assert!(!stripped.contains("panose"));
}

#[test]
fn plain_text_strips_apple_mail_cite_and_original_message() {
    let apple = "<div>Yes.</div><div><br><blockquote type=\"cite\"><div>On 2 Nov 2026, at 10:00, Kim wrote:</div><div>Question?</div></blockquote></div>";
    assert_eq!(to_plain_text(apple, true), "Yes.");
    let original =
        "<p>Done.</p><p>-----Original Message-----<br>From: Lee<br>Sent: Monday<br>Old text</p>";
    assert_eq!(to_plain_text(original, true), "Done.");
    let outlook_text = "<p>Ok</p><p>From: Lee &lt;lee@example.com&gt;<br>Sent: Monday<br>To: me<br>Subject: x</p><p>old</p>";
    assert_eq!(to_plain_text(outlook_text, true), "Ok");
}

#[test]
fn blocks_from_newsletter() {
    let blocks = to_blocks(&fixture("mailchimp_newsletter.html"));
    let flat = all_blocks(&blocks);
    assert!(flat.iter().any(|b| matches!(b, Block::Heading(1, inlines) if inline_text(inlines) == "Ethiopian naturals are back")));
    assert!(
        flat.iter()
            .any(|b| matches!(b, Block::List { ordered: false, items } if items.len() == 2))
    );
    assert!(flat.iter().any(|b| matches!(
        b,
        Block::Image { alt, src: ImageSource::Blocked(url) } if alt == "The Weekly Roast" && url.starts_with("https://mcusercontent.com/")
    )));
    assert!(!flat.iter().any(|b| matches!(b, Block::Image { src: ImageSource::Blocked(url), .. } if url.contains("list-manage"))));
    let paragraph = flat
        .iter()
        .find_map(|b| match b {
            Block::Paragraph(inlines) if inline_text(inlines).starts_with("Our Guji") => {
                Some(inlines.clone())
            }
            _ => None,
        })
        .expect("product paragraph");
    assert!(paragraph.contains(&Inline::Text {
        text: "Guji Hambela".into(),
        bold: true,
        italic: false,
        code: false
    }));
    assert!(paragraph.contains(&Inline::Text {
        text: "blueberry, jasmine and dark chocolate".into(),
        bold: false,
        italic: true,
        code: false
    }));
    let all_links = links(&blocks);
    assert!(all_links.contains(&(
        "Shop the Guji".into(),
        "https://shop.roastery.example/products/guji-hambela".into()
    )));
    assert!(all_links.iter().all(|(_, href)| !href.contains("utm_")));
    assert!(
        !flat
            .iter()
            .any(|b| matches!(b, Block::Paragraph(i) if inline_text(i).contains("brewing guide")))
    );
}

#[test]
fn blocks_allow_policy_reports_remote_images() {
    let blocks = to_blocks_with(
        &fixture("mailchimp_newsletter.html"),
        &Options::with_remote(RemotePolicy::Allow),
    );
    assert!(all_blocks(&blocks).iter().any(|b| matches!(b, Block::Image { src: ImageSource::Remote(url), .. } if url == "https://mcusercontent.com/abc123/images/logo.png")));
}

#[test]
fn blocks_from_gmail_reply_keep_quote() {
    let blocks = to_blocks(&fixture("gmail_reply.html"));
    let quote = blocks.iter().find_map(|b| match b {
        Block::Quote(inner) => Some(inner),
        _ => None,
    });
    let quote = quote.expect("quote block");
    assert!(
        quote
            .iter()
            .any(|b| matches!(b, Block::Paragraph(i) if inline_text(i).contains("Could we move")))
    );
    assert!(links(&blocks).contains(&(
        "calendar invite".into(),
        "https://calendar.example.com/event/123".into()
    )));
}

#[test]
fn blocks_from_receipt_flatten_tables_and_resolve_cids() {
    let html = fixture("receipt_cid.html");
    let default_blocks = to_blocks(&html);
    assert!(default_blocks.iter().any(|b| matches!(b, Block::Image { src: ImageSource::Cid(id), .. } if id == "logo@example.books")));
    let blocks = to_blocks_with(&html, &Options::default().with_cid_urls(receipt_cids()));
    assert!(blocks.iter().any(|b| matches!(b, Block::Image { src: ImageSource::Local(url), alt } if url == "schlau-cid://blob-logo" && alt == "Example Books")));
    assert!(blocks.iter().any(|b| matches!(b, Block::Image { src: ImageSource::Local(url), .. } if url == "schlau-cid://blob-item-2")));
    assert!(blocks.iter().any(
        |b| matches!(b, Block::Heading(2, i) if inline_text(i) == "Receipt #EB-20261107-0042")
    ));
    assert!(blocks.iter().any(
        |b| matches!(b, Block::Paragraph(i) if inline_text(i) == "The Rust Programming Language · 1 · $39.95")
    ));
    assert!(
        blocks.iter().any(
            |b| matches!(b, Block::Image { src: ImageSource::Inline(_), alt } if alt == "badge")
        )
    );
}

#[test]
fn blocks_from_hostile_email_are_safe() {
    let blocks = to_blocks(&fixture("hostile.html"));
    for (_, href) in links(&blocks) {
        assert!(
            href.starts_with("https://") || href.starts_with("mailto:"),
            "{href}"
        );
    }
    let flat = all_blocks(&blocks);
    assert!(!flat.iter().any(|b| matches!(b, Block::Paragraph(i) if inline_text(i).contains("alert(") && !inline_text(i).contains("is shown as text"))));
    assert!(!flat.iter().any(|b| matches!(b, Block::Image { src: ImageSource::Blocked(url), .. } if url.contains("track.gif"))));
}

#[test]
fn blocks_keep_pre_rule_and_ordered_lists() {
    let blocks = to_blocks(
        "<p>Steps:</p><ol><li>One</li><li><p>Two</p><pre>  code\n  block</pre></li></ol><hr><pre>\nfn main() {}\n</pre>",
    );
    assert_eq!(
        blocks,
        vec![
            Block::Paragraph(vec![Inline::Text {
                text: "Steps:".into(),
                bold: false,
                italic: false,
                code: false
            }]),
            Block::List {
                ordered: true,
                items: vec![
                    vec![Block::Paragraph(vec![Inline::Text {
                        text: "One".into(),
                        bold: false,
                        italic: false,
                        code: false
                    }])],
                    vec![
                        Block::Paragraph(vec![Inline::Text {
                            text: "Two".into(),
                            bold: false,
                            italic: false,
                            code: false
                        }]),
                        Block::Pre("  code\n  block".into()),
                    ],
                ],
            },
            Block::Rule,
            Block::Pre("fn main() {}".into()),
        ]
    );
}

#[test]
fn snippet_from_newsletter_text() {
    let text = to_plain_text(&fixture("mailchimp_newsletter.html"), true);
    let preview = snippet(&text, 80);
    assert!(preview.chars().count() <= 80);
    assert!(preview.ends_with('\u{2026}'));
    assert!(!preview.contains('\n'));
    let reply = snippet(&to_plain_text(&fixture("gmail_reply.html"), true), 200);
    assert!(reply.starts_with("Sounds good, Thursday at 3pm works for me."));
}
