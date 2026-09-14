//! CAN protocol core (PROMPT.md §6–§12).
//!
//! This module is intentionally independent of any MCU emulator (Renode),
//! frontend, or hardware interface. It models:
//!
//! - Level 1 — CAN frame: [`CanId`], [`CanFrame`] (ID / DLC / DATA)
//! - Level 2 — bit model: [`Bit`], [`crc`] (more wire codecs landing in
//!   Phase 2: stuffing, full frame encode/decode, error confinement)
//! - Bus + arbitration: [`CanBus`], [`arbitration`] (logical frame delivery)
//! - Timing: [`timing`] (simulation timestamps; no wall-clock dependence)
//!
//! The frame API stays stable and convenient while the bit model grows
//! underneath; the physical transceiver layer (§64) is layered on later.

pub mod arbitration;
pub mod bit;
pub mod bus;
pub mod crc;
pub mod frame;
pub mod id;
pub mod timing;

pub use arbitration::{arbitrate_ranking, arbitration_key, compare_frames};
pub use bit::{push_value_msb, read_value_msb, to_bit_string, Bit};
pub use bus::{
    ArbitrationOutcome, BusEvent, BusEventKind, CanBus, CanBusConfig, TransmissionOutcome,
};
pub use crc::{crc15, crc_matches, crc_to_bits, CRC_LEN, CRC_MASK, CRC_POLY};
pub use frame::{CanFormat, CanFrame, CanFrameError};
pub use id::{CanId, CanIdError};
pub use timing::SimNanos;
