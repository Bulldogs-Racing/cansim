//! CAN protocol core (PROMPT.md §6–§12).
//!
//! This module is intentionally independent of any MCU emulator (Renode),
//! frontend, or hardware interface. It models:
//!
//! - Level 1 — CAN frame: [`CanId`], [`CanFrame`] (ID / DLC / DATA)
//! - Bus + arbitration: [`CanBus`], [`arbitration`] (logical frame delivery)
//! - Timing: [`timing`] (simulation timestamps; no wall-clock dependence)
//!
//! Bit-level modeling (SOF/CRC/ACK/EOF, stuffing), error counters, and the
//! physical transceiver layer (§64) are layered on later — the frame API
//! stays stable and convenient.

pub mod arbitration;
pub mod bus;
pub mod frame;
pub mod id;
pub mod timing;

pub use arbitration::{arbitrate_ranking, arbitration_key, compare_frames};
pub use bus::{
    ArbitrationOutcome, BusEvent, BusEventKind, CanBus, CanBusConfig, TransmissionOutcome,
};
pub use frame::{CanFormat, CanFrame, CanFrameError};
pub use id::{CanId, CanIdError};
pub use timing::SimNanos;
