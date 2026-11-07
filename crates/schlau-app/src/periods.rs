//! The periods the conversation list is grouped by: today, yesterday, this week, this month, then
//! one per month. Rows come newest first, so each period is one run of rows.

use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    Today,
    Yesterday,
    /// Earlier this week, from Monday.
    ThisWeek,
    /// Earlier this month, before this week.
    ThisMonth,
    Month { year: i16, month: i8 },
}

/// A period and the rows in it, `start..end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Section {
    pub period: Period,
    /// The first day of the period.
    pub first_day: Date,
    pub start: usize,
    pub end: usize,
    pub unread: usize,
    /// The period's first second: a row at or after it, and before the previous period, is in it.
    from: i64,
}

impl Section {
    pub fn len(&self) -> usize {
        self.end - self.start
    }
}

const MONTHS: [&str; 12] =
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

fn month_name(month: i8) -> &'static str {
    MONTHS[(month.clamp(1, 12) - 1) as usize]
}

impl Period {
    /// The heading over the period's rows.
    pub fn title(self, today: Date) -> String {
        match self {
            Period::Today => "Today".into(),
            Period::Yesterday => "Yesterday".into(),
            Period::ThisWeek => "This week".into(),
            Period::ThisMonth => "This month".into(),
            Period::Month { year, month } if year == today.year() => month_name(month).into(),
            Period::Month { year, month } => format!("{} {year}", month_name(month)),
        }
    }

    /// The short name in the header's period strip.
    pub fn chip(self, today: Date) -> String {
        match self {
            Period::Month { year, month } if year == today.year() => month_name(month)[..3].into(),
            Period::Month { year, month } => format!("{} {year}", &month_name(month)[..3]),
            other => other.title(today),
        }
    }

    /// What the heading's dates are, beside it: the day, or where the period starts.
    pub fn dates(self, first_day: Date) -> String {
        match self {
            Period::Today | Period::Yesterday => first_day.strftime("%A, %-d %B").to_string(),
            Period::ThisWeek => first_day.strftime("since Monday %-d %b").to_string(),
            Period::ThisMonth => first_day.strftime("%B").to_string(),
            Period::Month { year, .. } => year.to_string(),
        }
    }

    /// A row's time inside its period: the clock for today and yesterday, the weekday this week,
    /// the weekday and day of the month before that.
    pub fn time_of(self, unix: i64, tz: &TimeZone) -> String {
        let Ok(at) = Timestamp::from_second(unix) else { return String::new() };
        let at = at.to_zoned(tz.clone());
        match self {
            Period::Today | Period::Yesterday => at.strftime("%H:%M").to_string(),
            Period::ThisWeek => at.strftime("%a %H:%M").to_string(),
            Period::ThisMonth | Period::Month { .. } => at.strftime("%a %-d").to_string(),
        }
    }
}

fn monday(day: Date) -> Date {
    day.checked_sub((day.weekday().to_monday_zero_offset() as i64).days()).unwrap_or(day)
}

/// The period `day` falls in, seen from `today`, and the period's first day.
fn period_of(day: Date, today: Date) -> (Period, Date) {
    let yesterday = today.yesterday().unwrap_or(today);
    let week = monday(today);
    let month = today.first_of_month();
    if day >= today {
        (Period::Today, today)
    } else if day == yesterday {
        (Period::Yesterday, yesterday)
    } else if day >= week {
        (Period::ThisWeek, week)
    } else if day >= month {
        (Period::ThisMonth, month)
    } else {
        (Period::Month { year: day.year(), month: day.month() }, day.first_of_month())
    }
}

fn start_of(day: Date, tz: &TimeZone) -> i64 {
    day.to_zoned(tz.clone()).map(|z| z.timestamp().as_second()).unwrap_or(i64::MIN)
}

/// The periods of rows given newest first as (latest time, unread), seen at `now` in `tz`.
pub fn sections(rows: impl IntoIterator<Item = (i64, bool)>, now: i64, tz: &TimeZone) -> Vec<Section> {
    let today = Timestamp::from_second(now).map(|t| t.to_zoned(tz.clone()).date()).unwrap_or_default();
    rows.into_iter().enumerate().fold(Vec::new(), |mut acc: Vec<Section>, (i, (at, unread))| {
        match acc.last_mut() {
            Some(last) if at >= last.from => {
                last.end = i + 1;
                last.unread += unread as usize;
            }
            _ => {
                let day = Timestamp::from_second(at).map(|t| t.to_zoned(tz.clone()).date()).unwrap_or(today);
                let (period, first_day) = period_of(day, today);
                acc.push(Section { period, first_day, start: i, end: i + 1, unread: unread as usize, from: start_of(first_day, tz) });
            }
        }
        acc
    })
}

/// The index of the section holding row `row`.
pub fn section_at(sections: &[Section], row: usize) -> usize {
    sections.partition_point(|s| s.start <= row).saturating_sub(1)
}

/// Where `]d` (`delta` 1) and `[d` (-1) take the selection from row `row`: the first row of the
/// next period, or back to the start of this one, then of the one before, as vim moves by sections.
pub fn jump_target(sections: &[Section], row: usize, delta: isize) -> Option<usize> {
    let at = section_at(sections, row);
    let current = sections.get(at)?;
    let target = match delta {
        d if d > 0 => at + 1,
        _ if row > current.start => at,
        _ => at.checked_sub(1)?,
    };
    sections.get(target).map(|s| s.start)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    fn unix(y: i16, m: i8, d: i8, h: i8) -> i64 {
        date(y, m, d).at(h, 0, 0, 0).to_zoned(TimeZone::UTC).unwrap().timestamp().as_second()
    }

    #[test]
    fn rows_fall_into_today_yesterday_week_month_and_months() {
        let now = unix(2026, 11, 7, 12);
        let at = [
            unix(2026, 11, 7, 11),
            unix(2026, 11, 7, 1),
            unix(2026, 11, 6, 20),
            unix(2026, 11, 5, 9),
            unix(2026, 11, 2, 0),
            unix(2026, 11, 1, 9),
            unix(2026, 10, 27, 9),
            unix(2026, 10, 1, 9),
            unix(2026, 9, 30, 9),
            unix(2025, 12, 24, 9),
        ];
        let rows = at.iter().enumerate().map(|(i, &t)| (t, i % 2 == 0));
        let found = sections(rows, now, &TimeZone::UTC);
        let shape: Vec<(Period, usize, usize, usize)> = found.iter().map(|s| (s.period, s.start, s.end, s.unread)).collect();
        assert_eq!(
            shape,
            [
                (Period::Today, 0, 2, 1),
                (Period::Yesterday, 2, 3, 1),
                (Period::ThisWeek, 3, 5, 1),
                (Period::ThisMonth, 5, 6, 0),
                (Period::Month { year: 2026, month: 10 }, 6, 8, 1),
                (Period::Month { year: 2026, month: 9 }, 8, 9, 1),
                (Period::Month { year: 2025, month: 12 }, 9, 10, 0),
            ]
        );
        let today = date(2026, 11, 7);
        assert_eq!(found[2].period.dates(found[2].first_day), "since Monday 2 Nov");
        assert_eq!(found[0].period.dates(found[0].first_day), "Saturday, 7 November");
        assert_eq!(found[4].period.title(today), "October");
        assert_eq!(found[6].period.title(today), "December 2025");
        assert_eq!(found[6].period.chip(today), "Dec 2025");
        assert_eq!(found[4].period.chip(today), "Oct");
    }

    #[test]
    fn a_week_that_began_last_month_leaves_no_this_month() {
        let now = unix(2026, 10, 1, 12);
        let rows = [unix(2026, 9, 30, 9), unix(2026, 9, 28, 9), unix(2026, 9, 27, 9)].map(|t| (t, false));
        let periods: Vec<Period> = sections(rows, now, &TimeZone::UTC).iter().map(|s| s.period).collect();
        assert_eq!(periods, [Period::Yesterday, Period::ThisWeek, Period::Month { year: 2026, month: 9 }]);
    }

    #[test]
    fn mail_from_the_future_counts_as_today() {
        let now = unix(2026, 11, 7, 12);
        let found = sections([(unix(2026, 11, 9, 9), true), (unix(2026, 11, 7, 9), false)], now, &TimeZone::UTC);
        assert_eq!(found.len(), 1);
        assert_eq!((found[0].period, found[0].len(), found[0].unread), (Period::Today, 2, 1));
    }

    #[test]
    fn jumps_go_to_period_starts_like_vim_sections() {
        let now = unix(2026, 11, 7, 12);
        let rows = [unix(2026, 11, 7, 11), unix(2026, 11, 7, 9), unix(2026, 11, 6, 9), unix(2026, 10, 2, 9), unix(2026, 10, 1, 9)];
        let s = sections(rows.map(|t| (t, false)), now, &TimeZone::UTC);
        assert_eq!(jump_target(&s, 0, 1), Some(2));
        assert_eq!(jump_target(&s, 1, 1), Some(2));
        assert_eq!(jump_target(&s, 2, 1), Some(3));
        assert_eq!(jump_target(&s, 4, 1), None, "nothing after the last period");
        assert_eq!(jump_target(&s, 4, -1), Some(3), "back to the start of this period first");
        assert_eq!(jump_target(&s, 3, -1), Some(2), "then to the previous one");
        assert_eq!(jump_target(&s, 0, -1), None);
        assert_eq!(jump_target(&[], 0, 1), None);
    }

    #[test]
    fn times_read_by_period() {
        let at = unix(2026, 11, 5, 9);
        assert_eq!(Period::Today.time_of(at, &TimeZone::UTC), "09:00");
        assert_eq!(Period::ThisWeek.time_of(at, &TimeZone::UTC), "Thu 09:00");
        assert_eq!(Period::ThisMonth.time_of(at, &TimeZone::UTC), "Thu 5");
    }
}
