use jiff::{Timestamp, tz::TimeZone};

/// A list date as mail clients show it: the time today, the day this year, the date before that.
pub fn short_date(unix: i64, now: i64) -> String {
    let tz = TimeZone::system();
    let (Ok(at), Ok(today)) = (Timestamp::from_second(unix), Timestamp::from_second(now)) else {
        return String::new();
    };
    let (at, today) = (at.to_zoned(tz.clone()), today.to_zoned(tz));
    if at.date() == today.date() {
        at.strftime("%H:%M").to_string()
    } else if at.year() == today.year() {
        at.strftime("%b %-d").to_string()
    } else {
        at.strftime("%Y-%m-%d").to_string()
    }
}

/// The full date in a message header.
pub fn long_date(unix: i64) -> String {
    Timestamp::from_second(unix)
        .map(|t| t.to_zoned(TimeZone::system()).strftime("%a, %b %-d %Y, %H:%M").to_string())
        .unwrap_or_default()
}

pub fn now_unix() -> i64 {
    Timestamp::now().as_second()
}
