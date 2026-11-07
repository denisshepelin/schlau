use schlau_store::Body;

/// The text the full-text index and the embedder see for a body: the plain part, else the HTML
/// part's text, with quoted replies left out so a thread doesn't index its history again and again.
pub fn body_index_text(body: &Body) -> String {
    match (&body.text, &body.html) {
        (Some(text), _) if !text.trim().is_empty() => strip_quoted_text(text),
        (_, Some(html)) => schlau_html::to_plain_text(html, true),
        _ => String::new(),
    }
}

fn strip_quoted_text(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let cut = lines
        .iter()
        .position(|l| {
            let l = l.trim();
            (l.starts_with("On ") && l.ends_with("wrote:")) || l == "-----Original Message-----"
        })
        .unwrap_or(lines.len());
    lines[..cut].iter().filter(|l| !l.trim_start().starts_with('>')).copied().collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_history_is_left_out() {
        let body = Body {
            text: Some("Sounds good.\n\nOn Mon, Jane wrote:\n> Lunch?\n> Thursday".into()),
            ..Default::default()
        };
        assert_eq!(body_index_text(&body).trim(), "Sounds good.");
    }

    #[test]
    fn html_only_bodies_index_their_text() {
        let body = Body { html: Some("<p>Hello <b>world</b></p><script>x()</script>".into()), ..Default::default() };
        assert_eq!(body_index_text(&body).trim(), "Hello world");
    }
}
