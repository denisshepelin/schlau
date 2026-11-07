use std::sync::LazyLock;

use markup5ever_rcdom::{Handle, NodeData};
use regex::Regex;

use crate::dom::{
    MAX_DEPTH, attribute, has_class, is_block, is_invisible_char, is_skipped, parse, tag_name,
    text_prefix,
};

/// Accumulates text with collapsed whitespace and bounded blank lines.
#[derive(Debug, Default)]
struct TextWriter {
    out: String,
    pending_space: bool,
    stopped: bool,
}

impl TextWriter {
    fn text(&mut self, text: &str, preformatted: bool) {
        text.chars()
            .filter(|c| !is_invisible_char(*c))
            .for_each(|c| {
                if preformatted {
                    self.out.push(if c == '\u{a0}' { ' ' } else { c });
                } else if c.is_whitespace() {
                    self.pending_space = true;
                } else {
                    if self.pending_space
                        && !self.out.is_empty()
                        && !self.out.ends_with(['\n', ' '])
                    {
                        self.out.push(' ');
                    }
                    self.pending_space = false;
                    self.out.push(c);
                }
            });
    }

    fn space(&mut self) {
        self.pending_space = true;
    }

    fn newlines(&mut self, count: usize) {
        self.pending_space = false;
        if self.out.is_empty() {
            return;
        }
        let trimmed_len = self.out.trim_end_matches([' ', '\t']).len();
        self.out.truncate(trimmed_len);
        let existing = self.out.len() - self.out.trim_end_matches('\n').len();
        (existing..count).for_each(|_| self.out.push('\n'));
    }

    fn literal(&mut self, text: &str) {
        self.out.push_str(text);
        self.pending_space = false;
    }
}

#[derive(Debug, Clone, Copy)]
struct Walk {
    strip_quoted: bool,
    preformatted: bool,
    depth: usize,
}

enum QuoteKind {
    /// Skip this subtree.
    Subtree,
    /// Everything from here to the end of the document is reply history.
    Rest,
}

static OUTLOOK_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:From|Von|De|Da|Van|Från|Fra|Od)\s*:")
        .expect("outlook header pattern compiles")
});

fn is_forwarded(node: &Handle) -> bool {
    let prefix = text_prefix(node, 400);
    prefix.contains("Forwarded message") || prefix.contains("Begin forwarded message")
}

fn quote_kind(node: &Handle) -> Option<QuoteKind> {
    let tag = tag_name(node)?;
    let id = attribute(node, "id").unwrap_or_default();
    if id == "appendonsend"
        || id == "divRplyFwdMsg"
        || id.ends_with("divRplyFwdMsg")
        || has_class(node, "OutlookMessageHeader")
    {
        return Some(QuoteKind::Rest);
    }
    let quoted_class = [
        "gmail_quote",
        "gmail_extra",
        "moz-cite-prefix",
        "yahoo_quoted",
        "protonmail_quote",
        "x_gmail_quote",
        "zmail_extra",
        "mail_android_quote",
    ]
    .iter()
    .any(|class| has_class(node, class));
    let cite = tag == "blockquote"
        && attribute(node, "type").is_some_and(|t| t.eq_ignore_ascii_case("cite"));
    if (quoted_class || cite) && !is_forwarded(node) {
        return Some(QuoteKind::Subtree);
    }
    let outlook_divider = tag == "div"
        && attribute(node, "style")
            .is_some_and(|style| style.to_ascii_lowercase().contains("border-top"))
        && OUTLOOK_HEADER.is_match(&text_prefix(node, 120));
    outlook_divider.then_some(QuoteKind::Rest)
}

fn walk(node: &Handle, writer: &mut TextWriter, ctx: Walk) {
    if writer.stopped || ctx.depth > MAX_DEPTH {
        return;
    }
    match &node.data {
        NodeData::Text { contents } => writer.text(&contents.borrow(), ctx.preformatted),
        NodeData::Element { .. } => walk_element(node, writer, ctx),
        NodeData::Document => walk_children(node, writer, ctx),
        _ => {}
    }
}

fn walk_children(node: &Handle, writer: &mut TextWriter, ctx: Walk) {
    let child_ctx = Walk {
        depth: ctx.depth + 1,
        ..ctx
    };
    node.children
        .borrow()
        .iter()
        .for_each(|child| walk(child, writer, child_ctx));
}

fn walk_element(node: &Handle, writer: &mut TextWriter, ctx: Walk) {
    if is_skipped(node) {
        return;
    }
    if ctx.strip_quoted {
        match quote_kind(node) {
            Some(QuoteKind::Subtree) => return,
            Some(QuoteKind::Rest) => {
                writer.stopped = true;
                return;
            }
            None => {}
        }
    }
    let tag = tag_name(node).unwrap_or_default();
    match tag {
        "br" => writer.newlines(1),
        "hr" => writer.newlines(2),
        "td" | "th" => {
            writer.space();
            walk_children(node, writer, ctx);
            writer.space();
        }
        "li" => {
            writer.newlines(1);
            writer.literal("- ");
            walk_children(node, writer, ctx);
            writer.newlines(1);
        }
        "p" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote" | "pre" | "ul" | "ol"
        | "table" => {
            writer.newlines(2);
            walk_children(
                node,
                writer,
                Walk {
                    preformatted: ctx.preformatted || tag == "pre",
                    ..ctx
                },
            );
            writer.newlines(2);
        }
        _ if is_block(tag) => {
            writer.newlines(1);
            walk_children(node, writer, ctx);
            writer.newlines(1);
        }
        _ => walk_children(node, writer, ctx),
    }
}

static ATTRIBUTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^\s*(?:on|am|le|el|il|op|em|w dniu|den|på|el día)\b.{0,300}\b(?:wrote|schrieb|a écrit|escribió|ha scritto|schreef|escreveu|napisał\(a\)|napisał|skrev)\s*:\s*$",
    )
    .expect("attribution pattern compiles")
});

static ORIGINAL_MESSAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*-{2,}\s*(?:original message|ursprüngliche nachricht|message d'origine|mensaje original|messaggio originale)\s*-{2,}")
        .expect("original message pattern compiles")
});

static HEADER_FOLLOW_UP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:sent|date|gesendet|datum|envoyé|enviado|inviato|verzonden|skickat|sendt|wysłano)\s*:")
        .expect("header follow-up pattern compiles")
});

fn starts_history(lines: &[&str], index: usize) -> bool {
    let line = lines[index];
    let joined = lines.get(index + 1).map(|next| format!("{line} {next}"));
    ATTRIBUTION.is_match(line)
        || joined.is_some_and(|joined| line.trim_start().len() > 2 && ATTRIBUTION.is_match(&joined))
        || ORIGINAL_MESSAGE.is_match(line)
        || (OUTLOOK_HEADER.is_match(line)
            && lines[index + 1..]
                .iter()
                .take(5)
                .any(|next| HEADER_FOLLOW_UP.is_match(next)))
}

/// Removes plain-text reply history: attribution lines and everything after, `>` quoted lines.
fn cut_reply_history(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let end = (0..lines.len())
        .find(|&i| starts_history(&lines, i))
        .unwrap_or(lines.len());
    lines[..end]
        .iter()
        .filter(|line| !line.trim_start().starts_with('>'))
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

fn tidy(text: &str) -> String {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let mut out = String::with_capacity(text.len());
    let mut blank_run = 0;
    for line in lines {
        if line.is_empty() {
            blank_run += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank_run > 0 { "\n\n" } else { "\n" });
        }
        blank_run = 0;
        out.push_str(line);
    }
    out
}

/// Readable text for search indexing and embeddings: block elements become line breaks,
/// `<head>`, styles, scripts and hidden elements are skipped, whitespace is collapsed.
/// With `strip_quoted`, reply history (Gmail, Apple Mail, Thunderbird, Outlook, and plain
/// "On ... wrote:" attributions) is dropped while forwarded messages are kept.
pub fn to_plain_text(html: &str, strip_quoted: bool) -> String {
    let dom = parse(html);
    let mut writer = TextWriter::default();
    walk(
        &dom.document,
        &mut writer,
        Walk {
            strip_quoted,
            preformatted: false,
            depth: 0,
        },
    );
    let text = if strip_quoted {
        cut_reply_history(&writer.out)
    } else {
        writer.out
    };
    tidy(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_become_lines() {
        let text = to_plain_text(
            "<html><head><title>T</title><style>p{}</style></head><body><p>One  two</p><div>three<br>four</div><ul><li>a</li><li>b</li></ul></body></html>",
            false,
        );
        assert_eq!(text, "One two\n\nthree\nfour\n\n- a\n- b");
    }

    #[test]
    fn strips_text_attribution() {
        let text = to_plain_text(
            "<p>Thanks!</p><p>On Mon, 3 Nov 2026 at 10:00, Ana &lt;ana@example.com&gt; wrote:</p><p>&gt; old</p>",
            true,
        );
        assert_eq!(text, "Thanks!");
    }

    #[test]
    fn keeps_forwarded_gmail_content() {
        let html = "<div>FYI</div><div class=\"gmail_quote\">---------- Forwarded message ---------<br>From: A<div>Body</div></div>";
        assert!(to_plain_text(html, true).contains("Body"));
    }
}
