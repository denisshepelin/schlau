//! Ranking for the command palette and highlighting of matched characters.

use rune::{Color, El, Span, TextStyle, rich};

/// Indices of `items` matching `query`, best first, with the matched character positions (byte
/// offsets). Typo-tolerant (fff's matcher). An empty query keeps the original order.
pub fn fuzzy(query: &str, items: &[&str], limit: usize) -> Vec<(usize, Vec<usize>)> {
    if query.trim().is_empty() {
        return (0..items.len().min(limit)).map(|i| (i, Vec::new())).collect();
    }
    schlau_search::fuzzy_rank(query.trim(), items, limit).into_iter().map(|m| (m.index, m.indices)).collect()
}

/// `title` with the characters at `positions` drawn in the accent colour.
pub fn highlighted(title: &str, positions: &[usize], style: TextStyle, ink: Color, accent: Color) -> El {
    let spans = positions
        .iter()
        .filter_map(|&p| title[p..].chars().next().map(|c| Span::new(p, p + c.len_utf8()).color(accent).weight(650.0)))
        .collect();
    rich(title.to_string(), spans).font(style).color(ink)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_starts_beat_scattered_letters() {
        let items = ["Go to Archive", "Mark as spam", "Go to Inbox"];
        let ranked = fuzzy("gi", &items, 10);
        assert_eq!(ranked[0].0, 2);
    }

    #[test]
    fn letters_out_of_order_do_not_match() {
        assert!(fuzzy("xz", &["Undo"], 10).is_empty());
    }
}
