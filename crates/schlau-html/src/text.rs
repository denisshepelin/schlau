use std::sync::LazyLock;

use regex::Regex;

use crate::links::clean_link_counted;
use crate::options::{BodyKind, RemotePolicy, Sanitized, Tally};
use crate::urls::escape_html;

/// How a `text/plain` body is encoded (RFC 3676).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextFormat {
    /// `format=flowed`: a line ending in a space continues on the next line.
    pub flowed: bool,
    /// `delsp=yes`: the trailing space of a soft line break is removed when joining.
    pub delsp: bool,
}

static LINK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(?:(?:https?://|www\.)[^\s<>\x22]+|mailto:[^\s<>\x22]+|[a-z0-9._%+\-]+@[a-z0-9\-]+(?:\.[a-z0-9\-]+)*\.[a-z]{2,})",
    )
    .expect("link pattern compiles")
});

#[derive(Debug, Clone, PartialEq, Eq)]
struct Line {
    depth: usize,
    text: String,
}

fn quote_depth(line: &str, flowed: bool) -> (usize, &str) {
    let mut depth = 0;
    let mut rest = line;
    loop {
        if let Some(next) = rest.strip_prefix('>') {
            depth += 1;
            rest = next;
        } else if !flowed && depth > 0 && rest.starts_with(" >") {
            rest = &rest[1..];
        } else {
            break;
        }
    }
    let unstuffed = if depth > 0 || flowed {
        rest.strip_prefix(' ').unwrap_or(rest)
    } else {
        rest
    };
    (depth, unstuffed)
}

fn is_soft_break(raw: &str) -> bool {
    raw.ends_with(' ') && raw != "-- "
}

fn parse_lines(plain: &str, format: TextFormat) -> Vec<Line> {
    let normalized = plain.replace("\r\n", "\n").replace('\r', "\n");
    normalized
        .split('\n')
        .fold(Vec::<(Line, bool)>::new(), |mut lines, raw| {
            let (depth, text) = quote_depth(raw, format.flowed);
            let soft = format.flowed && is_soft_break(text);
            let text = if soft && format.delsp {
                &text[..text.len() - 1]
            } else {
                text
            };
            match lines.last_mut() {
                Some((previous, open)) if *open && previous.depth == depth => {
                    previous.text.push_str(text);
                    *open = soft;
                }
                _ => lines.push((
                    Line {
                        depth,
                        text: text.to_owned(),
                    },
                    soft,
                )),
            }
            lines
        })
        .into_iter()
        .map(|(line, _)| line)
        .collect()
}

fn trim_link(candidate: &str) -> &str {
    let trimmed = candidate.trim_end_matches(['.', ',', ';', ':', '!', '?', '\'', '*']);
    let opens = trimmed.matches('(').count();
    let closes = trimmed.matches(')').count();
    if closes > opens && trimmed.ends_with(')') {
        trim_link(&trimmed[..trimmed.len() - 1])
    } else {
        trimmed
    }
}

fn link_target(text: &str) -> String {
    if text.contains("://") || text.to_ascii_lowercase().starts_with("mailto:") {
        text.to_owned()
    } else if text.to_ascii_lowercase().starts_with("www.") {
        format!("https://{text}")
    } else {
        format!("mailto:{text}")
    }
}

/// Escapes `text` and turns URLs and email addresses into cleaned links.
pub(crate) fn linkify(text: &str, tally: &mut Tally) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    let mut cursor = 0;
    for found in LINK.find_iter(text) {
        let shown = trim_link(found.as_str());
        if shown.is_empty() {
            continue;
        }
        let start = found.start();
        let end = start + shown.len();
        out.push_str(&escape_html(&text[cursor..start]));
        match clean_link_counted(&link_target(shown), tally) {
            Some(href) => out.push_str(&format!(
                "<a href=\"{}\" target=\"_blank\" rel=\"noopener noreferrer\">{}</a>",
                escape_html(&href),
                escape_html(shown)
            )),
            None => out.push_str(&escape_html(shown)),
        }
        cursor = end;
    }
    out.push_str(&escape_html(&text[cursor..]));
    out
}

fn render_lines(lines: &[Line], tally: &mut Tally) -> String {
    let mut out = String::with_capacity(lines.iter().map(|l| l.text.len() + 8).sum());
    let mut depth = 0;
    let mut in_signature = false;
    let mut first_in_block = true;
    for line in lines {
        if line.depth == 0 && line.text == "-- " && !in_signature {
            (0..depth).for_each(|_| out.push_str("</blockquote>"));
            depth = 0;
            out.push_str("<div class=\"schlau-signature\">");
            in_signature = true;
            first_in_block = true;
        }
        if line.depth != depth {
            (line.depth..depth).for_each(|_| out.push_str("</blockquote>"));
            (depth..line.depth).for_each(|_| out.push_str("<blockquote class=\"schlau-quote\">"));
            depth = line.depth;
            first_in_block = true;
        }
        if !first_in_block {
            out.push('\n');
        }
        out.push_str(&linkify(&line.text, tally));
        first_in_block = false;
    }
    (0..depth).for_each(|_| out.push_str("</blockquote>"));
    if in_signature {
        out.push_str("</div>");
    }
    out
}

/// Renders a `text/plain` body (format=fixed) as safe HTML.
pub fn text_to_html(plain: &str) -> Sanitized {
    text_to_html_with(plain, TextFormat::default())
}

/// Renders a `text/plain` body as safe HTML: escaped, URLs and addresses linkified with the
/// same cleaning as HTML links, `>` quote levels as nested blockquotes, flowed lines joined.
pub fn text_to_html_with(plain: &str, format: TextFormat) -> Sanitized {
    let mut tally = Tally::default();
    let lines = parse_lines(plain.trim_end_matches(['\n', '\r']), format);
    let body = render_lines(&lines, &mut tally);
    Sanitized {
        html: format!("<div class=\"schlau-plain\">{body}</div>"),
        report: tally.into_report(),
        remote: RemotePolicy::Block,
        kind: BodyKind::PlainText,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flowed_lines_join() {
        let lines = parse_lines(
            "Hello \nworld\n> quoted \n> more\nend",
            TextFormat {
                flowed: true,
                delsp: false,
            },
        );
        assert_eq!(
            lines,
            vec![
                Line {
                    depth: 0,
                    text: "Hello world".into()
                },
                Line {
                    depth: 1,
                    text: "quoted more".into()
                },
                Line {
                    depth: 0,
                    text: "end".into()
                },
            ]
        );
    }

    #[test]
    fn fixed_lines_keep_breaks() {
        let lines = parse_lines("a \nb\n> > c", TextFormat::default());
        assert_eq!(lines.len(), 3);
        assert_eq!(
            lines[2],
            Line {
                depth: 2,
                text: "c".into()
            }
        );
    }

    #[test]
    fn linkify_trims_punctuation_and_cleans() {
        let mut tally = Tally::default();
        let html = linkify(
            "See (https://example.com/a?utm_source=x). Mail bob@example.org!",
            &mut tally,
        );
        assert!(html.contains("href=\"https://example.com/a\""));
        assert!(html.contains(">https://example.com/a?utm_source=x</a>)."));
        assert!(html.contains("href=\"mailto:bob@example.org\""));
        assert_eq!(tally.params_stripped, 1);
    }
}
