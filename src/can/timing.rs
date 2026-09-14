//! Simulation timestamps (PROMPT.md §9).
//!
//! Time is an integer count of nanoseconds (`u64`). Floating-point time is
//! deliberately avoided for core scheduling so runs are deterministic:
//! the same project + seed + configuration yields the same event trace.

/// Nanoseconds since simulation start. Integer-based per §9.
pub type SimNanos = u64;

/// Number of nanoseconds in one millisecond (for display only).
pub const NS_PER_MS: u64 = 1_000_000;
/// Number of nanoseconds in one microsecond (for display only).
pub const NS_PER_US: u64 = 1_000;

pub fn format_ms(t_ns: SimNanos) -> String {
    format!("{:.3} ms", t_ns as f64 / NS_PER_MS as f64)
}
