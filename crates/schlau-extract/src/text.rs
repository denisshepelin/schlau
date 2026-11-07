use crate::{Extracted, Limits};

const ICS_FIELDS: [&str; 3] = ["SUMMARY", "DESCRIPTION", "LOCATION"];

/// Collapses runs of horizontal whitespace, trims lines and drops empty ones.
pub(crate) fn normalize_whitespace(raw: &str) -> String {
    raw.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

pub(crate) fn truncate_chars(mut text: String, max_chars: usize) -> String {
    if let Some((idx, _)) = text.char_indices().nth(max_chars) {
        text.truncate(idx);
    }
    text
}

/// Normalizes and truncates text; `None` when fewer than `min_chars` non-space characters remain.
pub(crate) fn finish(raw: &str, min_chars: usize, limits: &Limits) -> Option<String> {
    let text = truncate_chars(normalize_whitespace(raw), limits.max_text_chars)
        .trim_end()
        .to_owned();
    (text
        .chars()
        .filter(|c| !c.is_whitespace())
        .take(min_chars)
        .count()
        >= min_chars.max(1))
    .then_some(text)
}

pub(crate) fn extract(bytes: &[u8], is_calendar: bool, limits: &Limits) -> Extracted {
    let decoded = String::from_utf8_lossy(bytes);
    let raw = if is_calendar {
        calendar_text(&decoded)
    } else {
        decoded.into_owned()
    };
    Extracted {
        text: finish(&raw, 1, limits),
        images: Vec::new(),
    }
}

fn unfold(raw: &str) -> Vec<String> {
    raw.lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .fold(Vec::<String>::new(), |mut acc, line| {
            match (line.strip_prefix([' ', '\t']), acc.last_mut()) {
                (Some(rest), Some(prev)) => prev.push_str(rest),
                _ => acc.push(line.to_owned()),
            }
            acc
        })
}

fn property_value(line: &str) -> Option<(&str, &str)> {
    let colon = line
        .char_indices()
        .scan(false, |in_quotes, (idx, c)| {
            *in_quotes ^= c == '"';
            Some((idx, c, *in_quotes))
        })
        .find(|&(_, c, in_quotes)| c == ':' && !in_quotes)
        .map(|(idx, _, _)| idx)?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let name = head.split(';').next().unwrap_or_default();
    Some((name, value))
}

fn unescape_ics(value: &str) -> String {
    value
        .chars()
        .scan(false, |escaped, c| {
            let out = match (*escaped, c) {
                (false, '\\') => {
                    *escaped = true;
                    return Some(None);
                }
                (true, 'n' | 'N') => '\n',
                (_, c) => c,
            };
            *escaped = false;
            Some(Some(out))
        })
        .flatten()
        .collect()
}

fn calendar_text(raw: &str) -> String {
    unfold(raw)
        .iter()
        .filter_map(|line| property_value(line))
        .filter(|(name, _)| ICS_FIELDS.iter().any(|f| name.eq_ignore_ascii_case(f)))
        .map(|(_, value)| unescape_ics(value))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncates_on_char_boundary() {
        assert_eq!(truncate_chars("äöüß".to_owned(), 2), "äö");
        assert_eq!(truncate_chars("ab".to_owned(), 5), "ab");
    }

    #[test]
    fn normalizes_whitespace() {
        assert_eq!(normalize_whitespace("  a \t b\n\n\n c  "), "a b\nc");
    }

    #[test]
    fn unfolds_and_unescapes_calendar() {
        let ics = "BEGIN:VEVENT\r\nSUMMARY:Team\r\n  sync\r\nDESCRIPTION;LANGUAGE=en:line\\none\\, two\r\nLOCATION;ALTREP=\"http://x:1\":Room 1\r\nUID:abc\r\nEND:VEVENT\r\n";
        assert_eq!(calendar_text(ics), "Team sync\nline\none, two\nRoom 1");
    }
}
