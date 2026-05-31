//! Recurrence maths for reminders. Given a current deadline and a recurrence rule, compute the
//! next deadline. hours/days/weeks are plain arithmetic; months keep the anchor day-of-month,
//! clamped to the target month's length (so "31st" lands on the 28th/30th correctly).

use chrono::{Datelike, NaiveDate, TimeZone, Utc};

pub fn next_occurrence(freq: &str, every_n: i64, anchor_day: Option<i64>, after: i64) -> i64 {
    let n = every_n.max(1);
    match freq {
        "hours" => after + n * 3_600,
        "weeks" => after + n * 7 * 86_400,
        "months" => next_month(after, n, anchor_day),
        // "days" and anything unrecognised fall back to daily.
        _ => after + n * 86_400,
    }
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|d| d.pred_opt())
        .map(|d| d.day())
        .unwrap_or(28)
}

fn next_month(after: i64, n: i64, anchor_day: Option<i64>) -> i64 {
    let dt = Utc
        .timestamp_opt(after, 0)
        .single()
        .unwrap_or_else(Utc::now);
    // month0 is 0..=11, so absolute-month arithmetic is clean.
    let total = dt.year() as i64 * 12 + dt.month0() as i64 + n;
    let ny = total.div_euclid(12) as i32;
    let nm = total.rem_euclid(12) as u32 + 1;
    let want = anchor_day.unwrap_or(dt.day() as i64).clamp(1, 31) as u32;
    let day = want.min(days_in_month(ny, nm));
    match NaiveDate::from_ymd_opt(ny, nm, day).map(|d| d.and_time(dt.time())) {
        Some(naive) => Utc.from_utc_datetime(&naive).timestamp(),
        None => after + n * 30 * 86_400, // unreachable in practice
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone, Timelike, Utc};

    fn ts(y: i32, m: u32, d: u32, h: u32) -> i64 {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap().timestamp()
    }

    #[test]
    fn simple_intervals() {
        let t = ts(2026, 3, 10, 9);
        assert_eq!(next_occurrence("hours", 4, None, t), t + 4 * 3600);
        assert_eq!(next_occurrence("days", 1, None, t), t + 86_400);
        assert_eq!(next_occurrence("weeks", 2, None, t), t + 14 * 86_400);
    }

    #[test]
    fn monthly_clamps_to_month_end() {
        // Jan 31 -> Feb 28 (2026 is not a leap year), keeping the time of day.
        let next = next_occurrence("months", 1, Some(31), ts(2026, 1, 31, 9));
        let d = Utc.timestamp_opt(next, 0).single().unwrap();
        assert_eq!((d.year(), d.month(), d.day(), d.hour()), (2026, 2, 28, 9));
    }

    #[test]
    fn monthly_keeps_anchor_day() {
        // 28th each month: May 28 -> Jun 28.
        let next = next_occurrence("months", 1, Some(28), ts(2026, 5, 28, 8));
        let d = Utc.timestamp_opt(next, 0).single().unwrap();
        assert_eq!((d.year(), d.month(), d.day()), (2026, 6, 28));
    }

    #[test]
    fn monthly_wraps_year() {
        let next = next_occurrence("months", 2, Some(15), ts(2026, 11, 15, 0));
        let d = Utc.timestamp_opt(next, 0).single().unwrap();
        assert_eq!((d.year(), d.month(), d.day()), (2027, 1, 15));
    }
}
