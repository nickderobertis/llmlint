//! Delivery-slot scheduling. Every function takes the current time as an
//! argument (`now`, Unix seconds) so the rules are deterministic under test.

use std::collections::BTreeMap;

/// Seconds in one hour.
pub const HOUR: u64 = 3_600;
/// Seconds in one day.
pub const DAY: u64 = 24 * HOUR;

/// A bookable delivery window, `[start, end)` in Unix seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Slot {
    pub start: u64,
    pub end: u64,
}

impl Slot {
    /// The slot's length in seconds.
    pub fn duration(&self) -> u64 {
        self.end - self.start
    }

    /// Whether `instant` falls inside the slot (start inclusive, end exclusive).
    pub fn contains(&self, instant: u64) -> bool {
        self.start <= instant && instant < self.end
    }

    /// Whether this slot and `other` share any instant.
    pub fn overlaps(&self, other: &Slot) -> bool {
        self.start < other.end && other.start < self.end
    }
}

/// How far ahead, and how late in the day, customers may book.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BookingPolicy {
    /// A slot must start at least this many seconds after `now`.
    pub min_lead: u64,
    /// A slot may start at most this many seconds after `now`.
    pub max_horizon: u64,
    /// Slots per window before it is full.
    pub capacity: u32,
}

impl Default for BookingPolicy {
    fn default() -> Self {
        BookingPolicy { min_lead: 2 * HOUR, max_horizon: 14 * DAY, capacity: 20 }
    }
}

/// Generate back-to-back slots of `length` seconds covering `[from, until)`.
/// A trailing partial slot shorter than `length` is dropped.
pub fn generate_slots(from: u64, until: u64, length: u64) -> Vec<Slot> {
    if length == 0 {
        return Vec::new();
    }
    let mut slots = Vec::new();
    let mut start = from;
    while start + length <= until {
        slots.push(Slot { start, end: start + length });
        start += length;
    }
    slots
}

/// Whether `slot` can be booked at `now` under `policy`: it starts no sooner
/// than the minimum lead time and no later than the booking horizon.
pub fn is_bookable(slot: &Slot, now: u64, policy: &BookingPolicy) -> bool {
    slot.start >= now + policy.min_lead && slot.start <= now + policy.max_horizon
}

/// Booked-order counts per slot.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Bookings {
    counts: BTreeMap<Slot, u32>,
}

impl Bookings {
    /// Record one more order in `slot`.
    pub fn book(&mut self, slot: Slot) {
        *self.counts.entry(slot).or_insert(0) += 1;
    }

    /// How many orders are booked in `slot` (0 when none).
    pub fn booked(&self, slot: &Slot) -> u32 {
        self.counts.get(slot).copied().unwrap_or(0)
    }

    /// Remaining capacity in `slot` under `policy`, never below zero.
    pub fn remaining(&self, slot: &Slot, policy: &BookingPolicy) -> u32 {
        policy.capacity.saturating_sub(self.booked(slot))
    }
}

/// The slots a customer can pick right now: bookable at `now` and not full,
/// in start order.
pub fn open_slots(slots: &[Slot], bookings: &Bookings, now: u64, policy: &BookingPolicy) -> Vec<Slot> {
    let mut open: Vec<Slot> = slots
        .iter()
        .filter(|slot| is_bookable(slot, now, policy))
        .filter(|slot| bookings.remaining(slot, policy) > 0)
        .copied()
        .collect();
    open.sort();
    open
}

/// The earliest open slot at `now`, or `None` when every slot is full or out
/// of the booking window.
pub fn earliest_open(slots: &[Slot], bookings: &Bookings, now: u64, policy: &BookingPolicy) -> Option<Slot> {
    open_slots(slots, bookings, now, policy).into_iter().next()
}

/// Group slots by the day they start on (days counted from the Unix epoch).
pub fn slots_by_day(slots: &[Slot]) -> BTreeMap<u64, Vec<Slot>> {
    let mut days: BTreeMap<u64, Vec<Slot>> = BTreeMap::new();
    for slot in slots {
        days.entry(slot.start / DAY).or_default().push(*slot);
    }
    days
}

/// Fraction of total capacity already booked across `slots`, from 0.0 to 1.0.
/// Returns 0.0 for no slots or zero capacity.
pub fn utilization(slots: &[Slot], bookings: &Bookings, policy: &BookingPolicy) -> f64 {
    let capacity = u64::from(policy.capacity) * slots.len() as u64;
    if capacity == 0 {
        return 0.0;
    }
    let booked: u64 = slots
        .iter()
        .map(|slot| u64::from(bookings.booked(slot).min(policy.capacity)))
        .sum();
    booked as f64 / capacity as f64
}

/// Reasons a requested slot was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    TooSoon,
    TooFarAhead,
    Full,
}

/// Decide whether to accept a booking for `slot` at `now`, or why not.
pub fn check_booking(slot: &Slot, bookings: &Bookings, now: u64, policy: &BookingPolicy) -> Result<(), Refusal> {
    if slot.start < now + policy.min_lead {
        return Err(Refusal::TooSoon);
    }
    if slot.start > now + policy.max_horizon {
        return Err(Refusal::TooFarAhead);
    }
    if bookings.remaining(slot, policy) == 0 {
        return Err(Refusal::Full);
    }
    Ok(())
}

/// Customer-facing text for a refusal.
pub fn refusal_message(refusal: Refusal) -> &'static str {
    match refusal {
        Refusal::TooSoon => "That slot starts too soon; please pick a later one.",
        Refusal::TooFarAhead => "That slot is too far ahead to book yet.",
        Refusal::Full => "That slot is full.",
    }
}

/// The slots expiring soonest: open slots that stop being bookable within the
/// next `window` seconds, soonest first. Used to nudge customers.
pub fn closing_soon(slots: &[Slot], bookings: &Bookings, window: u64, policy: &BookingPolicy) -> Vec<Slot> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    open_slots(slots, bookings, now, policy)
        .into_iter()
        .filter(|slot| slot.start < now + policy.min_lead + window)
        .collect()
}

/// Shift every slot later by `offset` seconds (for example to move a whole
/// day's schedule after a holiday).
pub fn shift(slots: &[Slot], offset: u64) -> Vec<Slot> {
    slots
        .iter()
        .map(|slot| Slot { start: slot.start + offset, end: slot.end + offset })
        .collect()
}

/// Whether any two slots in `slots` overlap.
pub fn has_overlap(slots: &[Slot]) -> bool {
    let mut sorted = slots.to_vec();
    sorted.sort();
    sorted.windows(2).any(|pair| pair[0].overlaps(&pair[1]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_cover_the_range_without_a_partial_tail() {
        let slots = generate_slots(0, 5 * HOUR, 2 * HOUR);
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[1], Slot { start: 2 * HOUR, end: 4 * HOUR });
    }

    #[test]
    fn a_full_slot_is_refused() {
        let policy = BookingPolicy { capacity: 1, ..BookingPolicy::default() };
        let slot = Slot { start: 10 * HOUR, end: 11 * HOUR };
        let mut bookings = Bookings::default();
        bookings.book(slot);
        assert_eq!(check_booking(&slot, &bookings, 0, &policy), Err(Refusal::Full));
    }
}
