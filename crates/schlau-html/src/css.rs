use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

/// Where a piece of CSS lives; stylesheets get at-rule filtering and `</style>` escaping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CssContext {
    Inline,
    Sheet,
}

static DANGEROUS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)expression\s*\(|javascript\s*:|vbscript\s*:|behavior\s*:|-moz-binding|@import")
        .expect("dangerous css pattern compiles")
});

static AT_IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)@import\b(?:[^;"']|"[^"]*"|'[^']*')*(?:;|$)"#)
        .expect("import pattern compiles")
});

fn strip_comments(css: &str) -> Cow<'_, str> {
    if !css.contains("/*") {
        return Cow::Borrowed(css);
    }
    let mut out = String::with_capacity(css.len());
    let mut chars = css.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), _) if c == q => {
                quote = None;
                out.push(c);
            }
            (Some(_), '\\') => {
                out.push(c);
                out.extend(chars.next());
            }
            (Some(_), _) => out.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                out.push(c);
            }
            (None, '/') if chars.peek() == Some(&'*') => {
                chars.next();
                let mut previous = '\0';
                for next in chars.by_ref() {
                    if previous == '*' && next == '/' {
                        break;
                    }
                    previous = next;
                }
                out.push(' ');
            }
            (None, _) => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// Resolves CSS escapes so that `u\72l(` and `\65xpression(` are seen for what they are.
/// Escapes that would produce syntax characters are kept escaped.
fn unescape(css: &str) -> Cow<'_, str> {
    if !css.contains('\\') {
        return Cow::Borrowed(css);
    }
    let mut out = String::with_capacity(css.len());
    let mut chars = css.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let hex: String = std::iter::from_fn(|| chars.next_if(char::is_ascii_hexdigit))
            .take(6)
            .collect();
        let decoded = if hex.is_empty() {
            chars.next()
        } else {
            chars.next_if(|c| c.is_ascii_whitespace());
            u32::from_str_radix(&hex, 16)
                .ok()
                .and_then(char::from_u32)
                .or(Some('\u{fffd}'))
        };
        match decoded {
            Some(ch @ ('"' | '\'' | '\\' | '(' | ')' | ';' | '{' | '}' | '<' | '\n' | '\r')) => {
                out.push_str(&format!("\\{:x} ", ch as u32));
            }
            Some('\0') => out.push('\u{fffd}'),
            Some(ch) => out.push(ch),
            None => {}
        }
    }
    Cow::Owned(out)
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'-' || b == b'_'
}

fn find_function(css: &str, from: usize, name: &str) -> Option<usize> {
    let bytes = css.as_bytes();
    let needle = name.as_bytes();
    (from..bytes.len().saturating_sub(needle.len() - 1)).find(|&i| {
        bytes[i..i + needle.len()].eq_ignore_ascii_case(needle)
            && (i == 0 || !is_ident_byte(bytes[i - 1]))
    })
}

/// Index just past the `)` closing the call whose `(` ends at `open_end`.
fn matching_paren(css: &str, open_end: usize) -> usize {
    let mut depth = 1usize;
    let mut quote: Option<u8> = None;
    let bytes = css.as_bytes();
    let mut i = open_end;
    while i < bytes.len() {
        match (quote, bytes[i]) {
            (Some(q), b) if b == q => quote = None,
            (Some(_), b'\\') => i += 1,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(bytes[i]),
            (None, b'(') => depth += 1,
            (None, b')') => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    bytes.len()
}

fn url_argument(inner: &str) -> &str {
    let trimmed = inner.trim();
    match trimmed.as_bytes().first() {
        Some(q @ (b'"' | b'\'')) => {
            let body = &trimmed[1..];
            body.find(*q as char).map_or(body, |end| &body[..end])
        }
        _ => trimmed,
    }
}

fn css_string(url: &str) -> String {
    let escaped: String = url
        .chars()
        .flat_map(|c| match c {
            '"' | '\\' | '\n' | '\r' | '<' => {
                format!("\\{:x} ", c as u32).chars().collect::<Vec<_>>()
            }
            other => vec![other],
        })
        .collect();
    format!("url(\"{escaped}\")")
}

fn replace_calls(css: &str, name: &str, mut replace: impl FnMut(&str) -> String) -> String {
    let mut out = String::with_capacity(css.len());
    let mut cursor = 0;
    while let Some(start) = find_function(css, cursor, name) {
        let open_end = start + name.len();
        let end = matching_paren(css, open_end);
        out.push_str(&css[cursor..start]);
        out.push_str(&replace(
            &css[open_end..end.saturating_sub(1).max(open_end)],
        ));
        cursor = end;
    }
    out.push_str(&css[cursor..]);
    out
}

/// Rewrites every `url()` through `resolve` (`None` replaces it with `none`), removes
/// `@import` and `image-set()`, and neutralizes legacy script hooks.
pub(crate) fn rewrite_css(
    css: &str,
    context: CssContext,
    resolve: &mut dyn FnMut(&str) -> Option<String>,
) -> String {
    let cleaned = unescape(&strip_comments(css)).into_owned();
    let without_imports = AT_IMPORT.replace_all(&cleaned, "");
    let without_sets = ["-webkit-image-set(", "image-set("]
        .iter()
        .fold(without_imports.into_owned(), |acc, name| {
            replace_calls(&acc, name, |_| "none".into())
        });
    let with_urls = replace_calls(&without_sets, "url(", |inner| {
        resolve(url_argument(inner)).map_or_else(|| "none".to_owned(), |url| css_string(&url))
    });
    let neutral = DANGEROUS.replace_all(&with_urls, "blocked-");
    match context {
        CssContext::Inline => neutral.into_owned(),
        CssContext::Sheet => neutral.replace('<', "\\3c "),
    }
}

/// Lowercase `(property, value)` pairs of an inline style, for visibility checks.
pub(crate) fn declarations(style: &str) -> impl Iterator<Item = (String, String)> + '_ {
    style.split(';').filter_map(|declaration| {
        let (name, value) = declaration.split_once(':')?;
        Some((
            name.trim().to_ascii_lowercase(),
            value
                .trim()
                .trim_end_matches("!important")
                .trim()
                .to_ascii_lowercase(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite(css: &str) -> String {
        rewrite_css(css, CssContext::Sheet, &mut |url| {
            url.starts_with("data:").then(|| url.to_owned())
        })
    }

    #[test]
    fn replaces_remote_urls_with_none() {
        assert_eq!(
            rewrite("background: #fff url('https://t.example/p.gif') no-repeat"),
            "background: #fff none no-repeat"
        );
        assert_eq!(
            rewrite("a{background:URL( https://x/y )}"),
            "a{background:none}"
        );
    }

    #[test]
    fn keeps_allowed_urls_quoted() {
        assert_eq!(
            rewrite("b{background:url(data:image/png;base64,AA)}"),
            "b{background:url(\"data:image/png;base64,AA\")}"
        );
    }

    #[test]
    fn sees_through_escapes_and_comments() {
        assert_eq!(
            rewrite(r"p{background:u\72l(https://evil/x)}"),
            "p{background:none}"
        );
        assert_eq!(
            rewrite("p{background:/*x*/url(https://evil/x)}"),
            "p{background: none}"
        );
        assert!(
            !rewrite(r"p{width:\65 xpression(alert(1))}")
                .to_ascii_lowercase()
                .contains("expression(")
        );
    }

    #[test]
    fn drops_imports_and_image_sets() {
        assert!(
            !rewrite("@import 'https://evil/unterminated.css\np{}")
                .to_ascii_lowercase()
                .contains("@import")
        );
        assert_eq!(
            rewrite("@import url(https://evil/a.css); p{color:red}"),
            " p{color:red}"
        );
        assert_eq!(rewrite("@import 'https://evil/a.css'; p{}"), " p{}");
        assert_eq!(
            rewrite("p{background-image:image-set(\"https://e/a.png\" 1x)}"),
            "p{background-image:none}"
        );
    }

    #[test]
    fn escapes_style_breakout() {
        assert!(!rewrite("p{} </style><script>").contains("</style"));
    }
}
