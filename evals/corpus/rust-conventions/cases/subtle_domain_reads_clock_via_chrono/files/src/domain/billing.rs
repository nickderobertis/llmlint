use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};

/// A subscription's billing anchor: the day of month it renews on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subscription {
    pub anchor_day: u32,
}

/// The last day of `year`-`month` (28 to 31).
pub fn last_day_of_month(year: i32, month: u32) -> u32 {
    let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|first_of_next| first_of_next.pred_opt())
        .map(|last| last.day())
        .unwrap_or(28)
}

/// The renewal date in `year`-`month`: the anchor day, pulled back to the last
/// day of a shorter month.
pub fn renewal_in(subscription: &Subscription, year: i32, month: u32) -> Option<NaiveDate> {
    let day = subscription.anchor_day.min(last_day_of_month(year, month));
    NaiveDate::from_ymd_opt(year, month, day)
}

/// Days until the subscription's next renewal on or after `today`.
pub fn days_until_renewal(subscription: &Subscription, today: NaiveDate) -> i64 {
    let this_month = renewal_in(subscription, today.year(), today.month());
    let next_renewal = match this_month {
        Some(date) if date >= today => date,
        _ => {
            let (year, month) = if today.month() == 12 { (today.year() + 1, 1) } else { (today.year(), today.month() + 1) };
            renewal_in(subscription, year, month).unwrap_or(today)
        }
    };
    (next_renewal - today).num_days()
}

/// Whether a trial that started at `started` is still running.
pub fn trial_active(started: DateTime<Utc>, trial_length: Duration) -> bool {
    Utc::now() < started + trial_length
}
