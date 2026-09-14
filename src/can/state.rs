//! Fault confinement: TEC/REC counters and error states (PROMPT.md §12).
//!
//! Every CAN controller tracks a Transmit Error Counter (TEC) and a Receive
//! Error Counter (REC) driving three states:
//!
//! | State | Condition | Behaviour |
//! |---|---|---|
//! | Error active | TEC ≤ 127 and REC ≤ 127 | normal; signals active error flags |
//! | Error passive | TEC ≥ 128 or REC ≥ 128 | works on; passive flags only, TX suspend |
//! | Bus off | TEC ≥ 256 | detached; recovers after 128 × 11 recessive bits |
//!
//! Implemented counting rules (ISO 11898-1 core subset):
//!
//! - Transmitter sending an error flag: TEC += 8, **except** an
//!   error-passive transmitter seeing only an ACK error (no dominant bit
//!   while sending its passive flag): TEC unchanged, so a lone node parks
//!   at 128 instead of reaching bus-off on missing ACKs alone.
//! - Receiver detecting an error: REC += 1.
//! - Successful transmission: TEC −= 1 (floor 0).
//! - Successful reception: REC −= 1 when 1–127; stays 0 at 0; set to 127
//!   when above 127 (the spec allows 119–127; we pick the top
//!   deterministically).
//!
//! Deferred to later work (documented, not silent): the +8 special cases
//! around error/overload-flag bit sequences, overload frames, and the
//! error-passive transmit-suspend delay. The event stream carries every
//! transition, so those refine accounting without changing consumers.
//!
//! References: ISO 11898-1 § fault confinement; Bosch CAN 2.0;
//! Microchip 18C reference (TEC/REC rules); FlexCAN behaviour notes.

use super::errors::CanErrorKind;
use serde::{Deserialize, Serialize};
use std::fmt;

/// TEC/REC value at or above which a node turns error-passive.
pub const ERROR_PASSIVE_THRESHOLD: u16 = 128;
/// TEC value at or above which a node goes bus-off.
pub const BUS_OFF_THRESHOLD: u16 = 256;
/// Required 11-recessive-bit sequences observed while bus-off to recover.
pub const BUS_OFF_RECOVERY_SEQUENCES: u32 = 128;

/// Fault-confinement state of one CAN controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ErrorState {
    Active,
    Passive,
    BusOff,
}

impl fmt::Display for ErrorState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ErrorState::Active => "ERROR ACTIVE",
            ErrorState::Passive => "ERROR PASSIVE",
            ErrorState::BusOff => "BUS OFF",
        })
    }
}

/// Observable TEC/REC/state triple (§12 requires exposing exactly this).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControllerStatus {
    pub tec: u16,
    pub rec: u16,
    pub state: ErrorState,
}

/// Per-controller fault-confinement accounting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confinement {
    tec: u16,
    rec: u16,
    /// 11-recessive-bit sequences observed while bus-off.
    recovery_progress: u32,
}

impl Confinement {
    pub fn new() -> Self {
        Confinement {
            tec: 0,
            rec: 0,
            recovery_progress: 0,
        }
    }

    pub fn status(&self) -> ControllerStatus {
        ControllerStatus {
            tec: self.tec,
            rec: self.rec,
            state: self.state(),
        }
    }

    pub fn state(&self) -> ErrorState {
        if self.tec >= BUS_OFF_THRESHOLD {
            ErrorState::BusOff
        } else if self.tec >= ERROR_PASSIVE_THRESHOLD || self.rec >= ERROR_PASSIVE_THRESHOLD {
            ErrorState::Passive
        } else {
            ErrorState::Active
        }
    }

    /// Record a successful transmission. Returns the resulting state.
    pub fn note_tx_success(&mut self) -> ErrorState {
        self.tec = self.tec.saturating_sub(1);
        self.state()
    }

    /// Record a successful reception. Returns the resulting state.
    pub fn note_rx_success(&mut self) -> ErrorState {
        if self.rec > ERROR_PASSIVE_THRESHOLD - 1 {
            self.rec = ERROR_PASSIVE_THRESHOLD - 1; // deterministic pick in 119..=127
        } else {
            self.rec = self.rec.saturating_sub(1);
        }
        self.state()
    }

    /// Record a transmission-side error. Returns the resulting state.
    ///
    /// Applies the passive-ACK exception: an already-passive transmitter
    /// that only misses the acknowledgement does not count further.
    pub fn note_tx_error(&mut self, kind: CanErrorKind) -> ErrorState {
        let was_passive = self.state() != ErrorState::Active;
        if !(was_passive && kind == CanErrorKind::Ack) {
            self.tec = self.tec.saturating_add(8);
        }
        self.state()
    }

    /// Record a reception-side error detection. Returns the resulting state.
    pub fn note_rx_error(&mut self) -> ErrorState {
        self.rec = self.rec.saturating_add(1);
        self.state()
    }

    /// Observe one 11-consecutive-recessive-bit sequence (bus idle slice).
    /// While bus-off this advances recovery; the 128th sequence resets both
    /// counters to zero and returns the node to error-active. Returns
    /// `true` exactly on the recovering sequence.
    pub fn note_idle_11(&mut self) -> bool {
        if self.state() != ErrorState::BusOff {
            return false;
        }
        self.recovery_progress += 1;
        if self.recovery_progress >= BUS_OFF_RECOVERY_SEQUENCES {
            self.tec = 0;
            self.rec = 0;
            self.recovery_progress = 0;
            true
        } else {
            false
        }
    }

    /// Manual re-initialisation (CPU-driven, e.g. FlexCAN re-init after
    /// bus-off): counters cleared, recovery progress discarded.
    pub fn reset(&mut self) {
        self.tec = 0;
        self.rec = 0;
        self.recovery_progress = 0;
    }
}

impl Default for Confinement {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_controller_is_active_with_zero_counters() {
        let c = Confinement::new();
        assert_eq!(
            c.status(),
            ControllerStatus {
                tec: 0,
                rec: 0,
                state: ErrorState::Active
            }
        );
    }

    #[test]
    fn sixteen_tx_errors_reach_error_passive() {
        // Mirrors the field observation: 16 failed attempts × 8 = 128.
        let mut c = Confinement::new();
        for _ in 0..16 {
            c.note_tx_error(CanErrorKind::Ack);
        }
        assert_eq!(c.status().tec, 128);
        assert_eq!(c.status().state, ErrorState::Passive);
    }

    #[test]
    fn thirty_two_tx_errors_reach_bus_off() {
        let mut c = Confinement::new();
        for _ in 0..32 {
            c.note_tx_error(CanErrorKind::Bit);
        }
        assert!(c.status().tec >= BUS_OFF_THRESHOLD);
        assert_eq!(c.status().state, ErrorState::BusOff);
    }

    #[test]
    fn passive_sender_parks_on_pure_ack_errors() {
        // Error-passive + only missing ACKs: TEC must not advance further
        // (documented ISO exception; a lone node never ACKs itself bus-off).
        let mut c = Confinement::new();
        for _ in 0..16 {
            c.note_tx_error(CanErrorKind::Ack);
        }
        assert_eq!(c.status().state, ErrorState::Passive);
        for _ in 0..50 {
            c.note_tx_error(CanErrorKind::Ack);
        }
        assert_eq!(c.status().tec, 128);
        // Other error kinds still count while passive.
        c.note_tx_error(CanErrorKind::Bit);
        assert_eq!(c.status().tec, 136);
    }

    #[test]
    fn receive_errors_drive_passive_via_rec() {
        let mut c = Confinement::new();
        for _ in 0..128 {
            c.note_rx_error();
        }
        assert_eq!(c.status().rec, 128);
        assert_eq!(c.status().state, ErrorState::Passive);
    }

    #[test]
    fn successes_decrement_and_recover() {
        let mut c = Confinement::new();
        for _ in 0..5 {
            c.note_tx_error(CanErrorKind::Bit);
        }
        assert_eq!(c.status().tec, 40);
        c.note_tx_success();
        assert_eq!(c.status().tec, 39);

        for _ in 0..5 {
            c.note_rx_error();
        }
        c.note_rx_success();
        assert_eq!(c.status().rec, 4);

        // Above-127 REC snaps to 127 on success (spec range 119–127).
        for _ in 0..130 {
            c.note_rx_error();
        }
        c.note_rx_success();
        assert_eq!(c.status().rec, 127);
        assert_eq!(c.status().state, ErrorState::Active);
    }

    #[test]
    fn bus_off_recovers_after_128_idle_sequences() {
        let mut c = Confinement::new();
        for _ in 0..32 {
            c.note_tx_error(CanErrorKind::Bit);
        }
        assert_eq!(c.status().state, ErrorState::BusOff);
        for _ in 0..(BUS_OFF_RECOVERY_SEQUENCES - 1) {
            assert!(!c.note_idle_11());
            assert_eq!(c.status().state, ErrorState::BusOff);
        }
        assert!(c.note_idle_11());
        assert_eq!(c.status().state, ErrorState::Active);
        assert_eq!(c.status().tec, 0);
        assert_eq!(c.status().rec, 0);
    }

    #[test]
    fn idle_sequences_do_nothing_when_not_bus_off() {
        let mut c = Confinement::new();
        assert!(!c.note_idle_11());
        assert_eq!(c.status().state, ErrorState::Active);
    }
}
