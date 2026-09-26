//! Small shared helpers re-exported under crate-local names.

pub use std::fs::read_dir as list_entries;
pub use std::path::Path as FsPath;

/// Clamp `value` into `[low, high]`.
pub fn clamp(value: i64, low: i64, high: i64) -> i64 {
    value.max(low).min(high)
}
