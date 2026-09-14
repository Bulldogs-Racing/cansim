//! CAN protocol core (PROMPT.md §6–§12).
//!
//! This module is intentionally independent of any MCU emulator (Renode),
//! frontend, or hardware interface. It models:
//!
//! - Level 1 — CAN frame: [`CanId`], [`CanFrame`] (ID / DLC / DATA)
//! - Level 2 — bit model: [`Bit`], [`crc`], [`stuffing`] (frame
//!   encode/decode and error confinement landing next in Phase 2)
//! - Bus + arbitration: [`CanBus`], [`arbitration`] (logical frame delivery)
//! - Error handling: [`errors`], [`state`] (detection kinds, TEC/REC, states)
//! - Timing: [`timing`] (simulation timestamps; no wall-clock dependence)
//!
//! The frame API stays stable and convenient while the bit model grows
//! underneath; the physical transceiver layer (§64) is layered on later.

pub mod arbitration;
pub mod bit;
pub mod bits;
pub mod bus;
pub mod crc;
pub mod errors;
pub mod frame;
pub mod id;
pub mod state;
pub mod stuffing;
pub mod timing;

pub use arbitration::{arbitrate_ranking, arbitration_key, compare_frames};
pub use bit::{push_value_msb, read_value_msb, to_bit_string, Bit};
pub use bits::{
    decode_frame, encode_frame, wire_duration_ns, wire_len, DecodeError, DecodeInfo, EncodedFrame,
    EOF_LEN, IFS_LEN, TAIL_CRC_DELIM,
};
pub use bus::{
    ArbitrationOutcome, BusError, BusEvent, BusEventKind, CanBus, CanBusConfig, FaultOutcome,
    TransmissionOutcome, WireFault,
};
pub use crc::{crc15, crc_matches, crc_to_bits, CRC_LEN, CRC_MASK, CRC_POLY};
pub use errors::{
    kind_of_decode_error, CanErrorKind, ErrorFrame, ERROR_DELIMITER_LEN, ERROR_FLAG_LEN,
};
pub use frame::{CanFormat, CanFrame, CanFrameError};
pub use id::{CanId, CanIdError};
pub use state::{
    Confinement, ControllerStatus, ErrorState, BUS_OFF_RECOVERY_SEQUENCES, BUS_OFF_THRESHOLD,
    ERROR_PASSIVE_THRESHOLD,
};
pub use timing::SimNanos;
