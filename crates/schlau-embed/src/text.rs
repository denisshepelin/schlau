//! Turning an email into text to embed.
//!
//! Policy:
//! - The header is `subject` and `from`; it starts every passage so each passage
//!   carries the message's topic and sender.
//! - Quoted reply lines (starting with `>`) and everything after a `-- `
//!   signature delimiter are dropped: they repeat other messages or are boilerplate.
//! - Whitespace is collapsed. The body is split at word boundaries into
//!   passages of about [`ChunkPolicy::passage_tokens`] estimated tokens with a
//!   small overlap. Mean pooling over a whole newsletter averages unrelated
//!   topics into a vague vector; passages keep each one findable.
//! - At most [`ChunkPolicy::max_passages`] passages are produced; the rest of a
//!   very long body is not embedded.
//!
//! Tokens are estimated without the tokenizer: a CJK, kana or hangul character
//! counts as one token, any other character as a quarter, which tracks the
//! XLM-R sentencepiece vocabulary of the default model closely enough for budgeting.

/// Passage sizing for [`email_passages`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPolicy {
    /// Estimated token budget of one passage's body part.
    pub passage_tokens: usize,
    /// Estimated tokens repeated from the end of one passage at the start of the next.
    pub overlap_tokens: usize,
    /// Estimated token budget of the header (subject and sender).
    pub header_tokens: usize,
    pub max_passages: usize,
}

impl Default for ChunkPolicy {
    fn default() -> Self {
        Self {
            passage_tokens: 448,
            overlap_tokens: 32,
            header_tokens: 64,
            max_passages: 16,
        }
    }
}

/// One text for the whole message: the header and the start of the body, within
/// one passage's budget.
pub fn email_document(subject: &str, from: &str, body_text: &str) -> String {
    email_passages(subject, from, body_text, &ChunkPolicy::default())
        .into_iter()
        .next()
        .unwrap_or_default()
}

/// The message split into passages to embed separately. Always returns at least
/// one passage (the header alone when the body is empty).
pub fn email_passages(
    subject: &str,
    from: &str,
    body_text: &str,
    policy: &ChunkPolicy,
) -> Vec<String> {
    let header = header(subject, from, policy.header_tokens);
    let words = body_words(body_text);
    let chunks = chunk_words(&words, policy);
    if chunks.is_empty() {
        return vec![header];
    }
    chunks
        .into_iter()
        .map(|chunk| match header.is_empty() {
            true => chunk,
            false => format!("{header}\n\n{chunk}"),
        })
        .collect()
}

fn header(subject: &str, from: &str, budget: usize) -> String {
    let collapse = |s: &str| s.split_whitespace().collect::<Vec<_>>().join(" ");
    let joined = [collapse(subject), collapse(from)]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    truncate_to_tokens(&joined, budget).to_owned()
}

fn body_words(body: &str) -> Vec<&str> {
    body.lines()
        .take_while(|line| line.trim_end() != "--")
        .filter(|line| !line.trim_start().starts_with('>'))
        .flat_map(str::split_whitespace)
        .collect()
}

fn char_tokens(c: char) -> f32 {
    let wide = matches!(c as u32,
        0x1100..=0x11FF
        | 0x2E80..=0x9FFF
        | 0xAC00..=0xD7AF
        | 0xF900..=0xFAFF
        | 0xFF00..=0xFFEF
        | 0x20000..=0x3FFFF);
    if wide { 1.0 } else { 0.25 }
}

/// Estimated tokens of a word plus its separating space.
fn word_tokens(word: &str) -> f32 {
    word.chars().map(char_tokens).sum::<f32>() + 0.25
}

/// Estimated token count of `text`.
pub fn estimate_tokens(text: &str) -> usize {
    text.chars().map(char_tokens).sum::<f32>().ceil() as usize
}

fn truncate_to_tokens(text: &str, budget: usize) -> &str {
    let budget = budget as f32;
    let end = text
        .char_indices()
        .scan(0.0f32, |used, (i, c)| {
            *used += char_tokens(c);
            Some((i, *used))
        })
        .find(|&(_, used)| used > budget)
        .map_or(text.len(), |(i, _)| i);
    &text[..end]
}

/// The longest prefix of `text` within `budget`, but at least one character.
fn head(text: &str, budget: usize) -> &str {
    match truncate_to_tokens(text, budget) {
        "" => text.chars().next().map_or("", |c| &text[..c.len_utf8()]),
        prefix => prefix,
    }
}

/// Split words longer than the passage budget (unspaced CJK runs, URLs, base64)
/// into budget-sized pieces.
fn split_long_words<'a>(words: &[&'a str], budget: usize) -> Vec<&'a str> {
    words
        .iter()
        .flat_map(|&word| {
            std::iter::successors(Some(word), move |rest| {
                Some(&rest[head(rest, budget).len()..]).filter(|tail| !tail.is_empty())
            })
            .map(move |rest| head(rest, budget))
        })
        .collect()
}

fn chunk_words(words: &[&str], policy: &ChunkPolicy) -> Vec<String> {
    let budget = policy.passage_tokens.max(1);
    let words = split_long_words(words, budget);
    let costs: Vec<f32> = words.iter().map(|w| word_tokens(w)).collect();
    let (budget_f, overlap_f) = (budget as f32, policy.overlap_tokens as f32);
    let mut passages = Vec::new();
    let mut start = 0;
    while start < words.len() && passages.len() < policy.max_passages {
        let end = (start..words.len())
            .scan(0.0f32, |used, i| {
                *used += costs[i];
                Some((i, *used))
            })
            .find(|&(i, used)| used > budget_f && i > start)
            .map_or(words.len(), |(i, _)| i);
        passages.push(words[start..end].join(" "));
        if end == words.len() {
            break;
        }
        let overlap_start = (start + 1..end)
            .rev()
            .scan(0.0f32, |used, i| {
                *used += costs[i];
                Some((i, *used))
            })
            .take_while(|&(_, used)| used <= overlap_f)
            .last()
            .map_or(end, |(i, _)| i);
        start = overlap_start;
    }
    passages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_mail_is_one_passage_with_header() {
        let doc = email_document(
            "Invoice  March",
            "Acme <billing@acme.test>",
            "Please find\nthe bill attached.",
        );
        assert_eq!(
            doc,
            "Invoice March\nAcme <billing@acme.test>\n\nPlease find the bill attached."
        );
    }

    #[test]
    fn empty_body_yields_header_only() {
        assert_eq!(
            email_passages("Hi", "", "  \n", &ChunkPolicy::default()),
            vec!["Hi"]
        );
        assert_eq!(email_document("", "", ""), "");
    }

    #[test]
    fn quotes_and_signature_are_dropped() {
        let body = "Sounds good.\n> On Monday you wrote:\n> old text\nSee you.\n-- \nJane Doe\nCEO";
        assert_eq!(
            email_document("Re: plan", "Jane", body),
            "Re: plan\nJane\n\nSounds good. See you."
        );
    }

    #[test]
    fn long_body_is_chunked_with_overlap_and_cap() {
        let body: String = (0..3000).map(|i| format!("word{i:04} ")).collect();
        let policy = ChunkPolicy {
            passage_tokens: 100,
            overlap_tokens: 10,
            header_tokens: 16,
            max_passages: 5,
        };
        let passages = email_passages("Subject", "", &body, &policy);
        assert_eq!(passages.len(), 5);
        let bodies: Vec<Vec<&str>> = passages
            .iter()
            .map(|p| p.strip_prefix("Subject\n\n").unwrap().split(' ').collect())
            .collect();
        assert!(bodies.iter().all(|b| estimate_tokens(&b.join(" ")) <= 100));
        assert_eq!(bodies[0][0], "word0000");
        bodies.windows(2).for_each(|w| {
            let last = w[0].last().unwrap();
            assert!(w[1].contains(last), "consecutive passages overlap");
            assert_ne!(w[1][0], w[0][0]);
        });
    }

    #[test]
    fn whole_body_is_covered_without_cap() {
        let body: String = (0..500).map(|i| format!("w{i} ")).collect();
        let policy = ChunkPolicy {
            passage_tokens: 50,
            overlap_tokens: 0,
            ..ChunkPolicy::default()
        };
        let joined: Vec<String> = email_passages("", "", &body, &policy)
            .iter()
            .flat_map(|p| p.split(' ').map(str::to_owned).collect::<Vec<_>>())
            .collect();
        let expected: Vec<String> = (0..500).map(|i| format!("w{i}")).collect();
        assert_eq!(joined, expected);
    }

    #[test]
    fn unspaced_cjk_is_split_by_budget() {
        let body = "日本語のテキスト".repeat(200);
        let policy = ChunkPolicy {
            passage_tokens: 100,
            overlap_tokens: 0,
            ..ChunkPolicy::default()
        };
        let passages = email_passages("", "", &body, &policy);
        assert_eq!(passages.len(), 16);
        assert!(passages.iter().all(|p| p.chars().count() == 100));
        assert_eq!(estimate_tokens("abcd"), 1);
        assert_eq!(estimate_tokens("日本"), 2);
    }
}
