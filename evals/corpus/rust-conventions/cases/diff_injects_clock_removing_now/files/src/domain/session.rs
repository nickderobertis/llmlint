use std::time::{Duration, SystemTime};

/// A login session and when it was last active.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub user_id: u64,
    pub last_active: SystemTime,
}

/// Sessions idle longer than this are expired.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// Whether `session` has been idle longer than `IDLE_TIMEOUT` as of `now`. A
/// `last_active` later than `now` counts as not idle.
pub fn is_idle(session: &Session, now: SystemTime) -> bool {
    now.duration_since(session.last_active)
        .map(|idle| idle > IDLE_TIMEOUT)
        .unwrap_or(false)
}
