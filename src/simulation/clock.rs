//! Deterministic simulation clock (PROMPT.md §9).
//!
//! The engine never depends on wall-clock timing: [`SimClock`] is an integer
//! nanosecond counter supporting pause, step, reset, and accelerated runs.
//! Identical projects stepped identically produce identical traces.

use crate::can::timing::SimNanos;
use serde::{Deserialize, Serialize};

/// Integer-nanosecond simulation clock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimClock {
    now_ns: SimNanos,
}

impl SimClock {
    pub fn new() -> Self {
        SimClock { now_ns: 0 }
    }

    pub fn now(&self) -> SimNanos {
        self.now_ns
    }

    /// Move forward by `delta_ns`. Time never goes backwards.
    pub fn advance(&mut self, delta_ns: SimNanos) {
        self.now_ns = self.now_ns.saturating_add(delta_ns);
    }

    /// Jump to an absolute timestamp; rejects going backwards to keep runs
    /// deterministic and monotonic.
    pub fn set(&mut self, t_ns: SimNanos) -> Result<(), ClockError> {
        if t_ns < self.now_ns {
            return Err(ClockError::BackwardJump {
                got: t_ns,
                now: self.now_ns,
            });
        }
        self.now_ns = t_ns;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.now_ns = 0;
    }
}

impl Default for SimClock {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ClockError {
    #[error("clock cannot go backwards: now={now} ns, requested={got} ns")]
    BackwardJump { got: SimNanos, now: SimNanos },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_and_reset() {
        let mut c = SimClock::new();
        assert_eq!(c.now(), 0);
        c.advance(1_000);
        assert_eq!(c.now(), 1_000);
        assert!(c.set(500).is_err());
        c.set(2_000).unwrap();
        c.reset();
        assert_eq!(c.now(), 0);
    }
}
