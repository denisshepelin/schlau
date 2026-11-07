//! Operator names and the values they take: dates, sizes, `is:`/`has:` flags.

use crate::query::{Field, Filter};

const DAY: i64 = 86_400;

/// A recognized operator (the part before `:`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    Text(Field),
    Mailbox,
    Is,
    Has,
    After,
    Before,
    OlderThan,
    NewerThan,
    Larger,
    Smaller,
}

impl Op {
    pub(crate) fn from_name(name: &str) -> Option<Op> {
        let op = match name.to_ascii_lowercase().as_str() {
            "from" => Op::Text(Field::From),
            "to" => Op::Text(Field::To),
            "cc" => Op::Text(Field::Cc),
            "subject" => Op::Text(Field::Subject),
            "body" => Op::Text(Field::Body),
            "filename" | "attachment" => Op::Text(Field::Attachment),
            "in" | "label" => Op::Mailbox,
            "is" => Op::Is,
            "has" => Op::Has,
            "after" | "since" => Op::After,
            "before" | "until" => Op::Before,
            "older_than" | "older" => Op::OlderThan,
            "newer_than" | "newer" => Op::NewerThan,
            "larger" | "bigger" => Op::Larger,
            "smaller" => Op::Smaller,
            _ => return None,
        };
        Some(op)
    }

    /// The filter a value of this operator stands for. `None` when the value is not valid for the operator.
    /// Text operators never produce filters.
    pub(crate) fn filter(self, value: &str, clock: Clock) -> Option<Filter> {
        match self {
            Op::Text(_) => None,
            Op::Mailbox => (!value.trim().is_empty()).then(|| Filter::InMailbox(value.to_owned())),
            Op::Is => is_flag(value),
            Op::Has => has_flag(value),
            Op::After => date(value, clock).map(Filter::After),
            Op::Before => date(value, clock).map(Filter::Before),
            Op::OlderThan => relative(value).map(|d| Filter::Before(clock.now - d)),
            Op::NewerThan => relative(value).map(|d| Filter::After(clock.now - d)),
            Op::Larger => size(value).map(Filter::Larger),
            Op::Smaller => size(value).map(Filter::Smaller),
        }
    }
}

/// The current time and the user's offset from UTC, for dates like `today` and `2026-01-31`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Clock {
    pub now: i64,
    pub utc_offset: i64,
}

impl Clock {
    fn day_start(self, days_back: i64) -> i64 {
        let local = self.now + self.utc_offset;
        (local.div_euclid(DAY) - days_back) * DAY - self.utc_offset
    }

    fn civil_start(self, year: i64, month: u32, day: u32) -> i64 {
        days_from_civil(year, month, day) * DAY - self.utc_offset
    }
}

fn is_flag(value: &str) -> Option<Filter> {
    let f = match value.to_ascii_lowercase().as_str() {
        "unread" | "unseen" => Filter::Unread,
        "read" | "seen" => Filter::Read,
        "flagged" | "starred" | "pinned" => Filter::Flagged,
        "draft" | "drafts" => Filter::Draft,
        "answered" | "replied" => Filter::Answered,
        _ => return None,
    };
    Some(f)
}

fn has_flag(value: &str) -> Option<Filter> {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "attachment" | "attachments" | "file" | "files"
    )
    .then_some(Filter::HasAttachment)
}

/// `today`, `yesterday`, a relative age (`7d`), or a calendar date (`2026-01-31`, `2026/1/31`, `2026-01`,
/// `2026`) as the start of that day, month or year in the user's time zone.
fn date(value: &str, clock: Clock) -> Option<i64> {
    match value.to_ascii_lowercase().as_str() {
        "today" => Some(clock.day_start(0)),
        "yesterday" => Some(clock.day_start(1)),
        v => relative(v)
            .map(|d| clock.now - d)
            .or_else(|| calendar(v, clock)),
    }
}

fn calendar(value: &str, clock: Clock) -> Option<i64> {
    let parts: Vec<&str> = value.split(['-', '/', '.']).collect();
    let num = |s: &str, max_len: usize| -> Option<u32> {
        (!s.is_empty() && s.len() <= max_len && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    let (year, month, day) = match parts.as_slice() {
        [y] if y.len() == 4 => (num(y, 4)?, 1, 1),
        [y, m] if y.len() == 4 => (num(y, 4)?, num(m, 2)?, 1),
        [y, m, d] if y.len() == 4 => (num(y, 4)?, num(m, 2)?, num(d, 2)?),
        _ => return None,
    };
    let valid = (1970..=9999).contains(&year)
        && (1..=12).contains(&month)
        && (1..=days_in_month(i64::from(year), month)).contains(&day);
    valid.then(|| clock.civil_start(i64::from(year), month, day))
}

/// A relative age in seconds: `30h`, `7d`, `2w`, `3m` (30 days), `1y` (365 days).
fn relative(value: &str) -> Option<i64> {
    let unit = value.chars().last()?.to_ascii_lowercase();
    let digits = &value[..value.len() - unit.len_utf8()];
    if digits.is_empty() || digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: i64 = digits.parse().ok()?;
    let seconds = match unit {
        'h' => 3_600,
        'd' => DAY,
        'w' => 7 * DAY,
        'm' => 30 * DAY,
        'y' => 365 * DAY,
        _ => return None,
    };
    Some(n * seconds)
}

/// A size in bytes: `500`, `100k`, `1.5M`, `2GB`. Units are binary (k = 1024).
fn size(value: &str) -> Option<u64> {
    let lower = value.to_ascii_lowercase();
    let split = lower
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(lower.len());
    let (number, unit) = lower.split_at(split);
    let multiplier: f64 = match unit {
        "" | "b" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let n: f64 = number.parse().ok().filter(|n: &f64| n.is_finite())?;
    let bytes = n * multiplier;
    (0.0..1e15).contains(&bytes).then_some(bytes.round() as u64)
}

fn days_in_month(year: i64, month: u32) -> u32 {
    match month {
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    const UTC: Clock = Clock {
        now: 1_767_225_600 + 12 * 3600,
        utc_offset: 0,
    };

    #[test]
    fn civil_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2026, 1, 1) * DAY, 1_767_225_600);
    }

    #[test]
    fn calendar_formats() {
        assert_eq!(date("2026-01-01", UTC), Some(1_767_225_600));
        assert_eq!(date("2026/01/01", UTC), Some(1_767_225_600));
        assert_eq!(date("2026/1/1", UTC), Some(1_767_225_600));
        assert_eq!(date("2026-01", UTC), Some(1_767_225_600));
        assert_eq!(date("2026", UTC), Some(1_767_225_600));
        assert_eq!(
            date("2024-02-29", UTC),
            Some(days_from_civil(2024, 2, 29) * DAY)
        );
    }

    #[test]
    fn invalid_dates() {
        [
            "2025-02-29",
            "2026-13-01",
            "2026-00-10",
            "2026-01-32",
            "26-01-01",
            "abc",
            "",
            "2026--01",
            "1-2-3",
        ]
        .iter()
        .for_each(|v| assert_eq!(date(v, UTC), None, "{v}"));
    }

    #[test]
    fn today_and_yesterday_respect_offset() {
        assert_eq!(date("today", UTC), Some(1_767_225_600));
        assert_eq!(date("Yesterday", UTC), Some(1_767_225_600 - DAY));
        let tokyo = Clock {
            now: 1_767_225_600 - 3600,
            utc_offset: 9 * 3600,
        };
        assert_eq!(date("today", tokyo), Some(1_767_225_600 - 9 * 3600));
    }

    #[test]
    fn relative_ages() {
        assert_eq!(relative("7d"), Some(7 * DAY));
        assert_eq!(relative("2w"), Some(14 * DAY));
        assert_eq!(relative("3M"), Some(90 * DAY));
        assert_eq!(relative("1y"), Some(365 * DAY));
        assert_eq!(relative("12h"), Some(12 * 3600));
        ["d", "7", "7x", "-7d", "7dd", "1234567d", ""]
            .iter()
            .for_each(|v| assert_eq!(relative(v), None, "{v}"));
    }

    #[test]
    fn sizes() {
        assert_eq!(size("500"), Some(500));
        assert_eq!(size("100k"), Some(102_400));
        assert_eq!(size("5M"), Some(5 * 1024 * 1024));
        assert_eq!(size("1.5mb"), Some(1_572_864));
        assert_eq!(size("2GB"), Some(2 * 1024 * 1024 * 1024));
        ["", "M", "5T", "1.2.3k", "abc", "5 M"]
            .iter()
            .for_each(|v| assert_eq!(size(v), None, "{v}"));
    }

    #[test]
    fn op_names_are_case_insensitive() {
        assert_eq!(Op::from_name("FROM"), Some(Op::Text(Field::From)));
        assert_eq!(Op::from_name("Label"), Some(Op::Mailbox));
        assert_eq!(Op::from_name("nope"), None);
    }
}
