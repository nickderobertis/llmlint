/// Request counters for one endpoint over a reporting window.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Counters {
    pub requests: u64,
    pub errors: u64,
    pub slow_requests: u64,
}

impl Counters {
    /// The share of requests that failed, from 0.0 to 1.0 (0.0 with no
    /// requests).
    pub fn error_rate(&self) -> f64 {
        if self.requests == 0 {
            return 0.0;
        }
        self.errors as f64 / self.requests as f64
    }
}

/// Sum two windows' counters field by field.
pub fn merge_counters(earlier: Counters, val: Counters) -> Counters {
    Counters {
        requests: earlier.requests + val.requests,
        errors: earlier.errors + val.errors,
        slow_requests: earlier.slow_requests + val.slow_requests,
    }
}

/// Whether `window` breaches the error budget of `max_error_rate`.
pub fn breaches_budget(window: &Counters, max_error_rate: f64) -> bool {
    window.error_rate() > max_error_rate
}

/// The indices of the windows in `history` that breached the budget, in
/// history order.
pub fn breaching_windows(history: &[Counters], max_error_rate: f64) -> Vec<usize> {
    history
        .iter()
        .enumerate()
        .filter(|(_, window)| breaches_budget(window, max_error_rate))
        .map(|(position, _)| position)
        .collect()
}
