//! Classical CAN frame representation (PROMPT.md §6).
//!
//! Design notes / protocol assumptions (documented per §11):
//!
//! - Classical CAN only in the MVP. [`CanFormat::Fd`] exists so future CAN FD
//!   support is additive, but [`CanFrame::new_fd`] explicitly returns
//!   [`CanFrameError::FdNotSupported`] — never a silent fake (§72).
//! - `dlc` is the data length code 0..=8 for Classical CAN. Raw on-wire DLC
//!   values 9..=15 (which denote 8 bytes on real hardware) are rejected at
//!   this layer for clarity; a future bit-level decoder (§10) can map them.
//! - Data frames carry `data.len() == dlc`. Remote (RTR) frames carry no
//!   payload; `dlc` is the *requested* length (0..=8).
//! - Payload storage is a `Vec<u8>` capped at 8 bytes; constructors validate
//!   so invalid frames cannot be constructed.

use super::id::CanId;
use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Maximum Classical CAN payload in bytes.
pub const MAX_CLASSICAL_DLC: u8 = 8;

/// Wire format of the frame. Only [`CanFormat::Classical`] is simulated in
/// the MVP (§37); `Fd` is a typed placeholder for the future extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum CanFormat {
    #[default]
    Classical,
    /// Reserved for future CAN FD support (up to 64 bytes, BRS/ESI).
    /// Constructing such a frame currently fails with `FdNotSupported`.
    Fd,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CanFrameError {
    #[error("DLC {0} out of range for Classical CAN (must be 0..=8)")]
    InvalidDlc(u8),
    #[error("data length {actual} does not match DLC {dlc} for a data frame")]
    LengthMismatch { dlc: u8, actual: usize },
    #[error("remote (RTR) frame must carry no data, got {0} bytes")]
    RemoteWithData(usize),
    #[error("CAN FD is not implemented yet (PROMPT §37); Classical CAN only")]
    FdNotSupported,
}

/// Strongly-typed Classical CAN frame: ID + RTR flag + DLC + payload.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CanFrame {
    pub id: CanId,
    /// `true` for a remote transmission request (RTR) frame.
    pub is_remote: bool,
    /// Data length code (0..=8). For remote frames: requested length.
    pub dlc: u8,
    /// Payload bytes. Empty for remote frames.
    pub data: Vec<u8>,
    pub format: CanFormat,
}

impl CanFrame {
    /// Build a Classical data frame, validating DLC/payload consistency.
    pub fn new(id: CanId, data: &[u8]) -> Result<Self, CanFrameError> {
        if data.len() > MAX_CLASSICAL_DLC as usize {
            return Err(CanFrameError::InvalidDlc(data.len() as u8));
        }
        Ok(CanFrame {
            id,
            is_remote: false,
            dlc: data.len() as u8,
            data: data.to_vec(),
            format: CanFormat::Classical,
        })
    }

    /// Build a Classical remote (RTR) frame requesting `requested_dlc` bytes.
    pub fn new_remote(id: CanId, requested_dlc: u8) -> Result<Self, CanFrameError> {
        if requested_dlc > MAX_CLASSICAL_DLC {
            return Err(CanFrameError::InvalidDlc(requested_dlc));
        }
        Ok(CanFrame {
            id,
            is_remote: true,
            dlc: requested_dlc,
            data: Vec::new(),
            format: CanFormat::Classical,
        })
    }

    /// Explicit FD constructor — always fails until §37 CAN FD lands.
    pub fn new_fd(_id: CanId, _data: &[u8]) -> Result<Self, CanFrameError> {
        Err(CanFrameError::FdNotSupported)
    }

    /// Hex rendering of the payload, e.g. `"01 02 03 04"`.
    pub fn data_hex(&self) -> String {
        self.data
            .iter()
            .map(|b| format!("{:02X}", b))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Nominal frame length on the wire in bits, *excluding* bit stuffing.
    /// This is a planning estimate only; use
    /// [`wire_len`](super::bits::wire_len) for the exact stuffed length.
    ///
    /// `SOF(1) + ID(11/29 + SRR/IDE extras) + control(6) + data(8*dlc)
    ///  + CRC(16) + ACK(2) + EOF(7) + IFS(3)`.
    pub fn nominal_bit_len(&self) -> usize {
        let id_bits = match self.id {
            CanId::Standard(_) => 11 + 1,              // 11 ID + RTR
            CanId::Extended(_) => 11 + 1 + 1 + 18 + 1, // base + SRR + IDE + ext + RTR
        };
        1 + id_bits + 6 + (self.dlc as usize * 8) + 16 + 2 + 7 + 3
    }

    /// Nominal transmission time in nanoseconds at `bitrate_bit_s`
    /// (estimate; see [`wire_duration_ns`](super::bits::wire_duration_ns)
    /// for the exact stuffed timing the engine steps).
    pub fn nominal_duration_ns(&self, bitrate_bit_s: u32) -> u64 {
        assert!(bitrate_bit_s > 0, "bitrate must be positive");
        self.nominal_bit_len() as u64 * 1_000_000_000 / bitrate_bit_s as u64
    }
}

impl fmt::Display for CanFrame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_remote {
            write!(f, "{} RTR dlc={}", self.id, self.dlc)
        } else {
            write!(f, "{} [{}]", self.id, self.data_hex())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_frame_round_trip() {
        let id = CanId::new_standard(0x123).unwrap();
        let frame = CanFrame::new(id, &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08]).unwrap();
        assert_eq!(frame.dlc, 8);
        assert_eq!(frame.data_hex(), "01 02 03 04 05 06 07 08");
    }

    #[test]
    fn dlc_validation() {
        let id = CanId::new_standard(0x100).unwrap();
        assert_eq!(
            CanFrame::new(id, &[0; 9]),
            Err(CanFrameError::InvalidDlc(9))
        );
        assert_eq!(
            CanFrame::new_remote(id, 9),
            Err(CanFrameError::InvalidDlc(9))
        );
    }

    #[test]
    fn remote_frame_has_no_payload() {
        let id = CanId::new_standard(0x200).unwrap();
        let rtr = CanFrame::new_remote(id, 4).unwrap();
        assert!(rtr.is_remote);
        assert!(rtr.data.is_empty());
        assert_eq!(rtr.dlc, 4);
    }

    #[test]
    fn fd_is_explicitly_unsupported() {
        let id = CanId::new_standard(0x100).unwrap();
        assert_eq!(
            CanFrame::new_fd(id, &[1, 2, 3]),
            Err(CanFrameError::FdNotSupported)
        );
    }

    #[test]
    fn extended_and_standard_ids_coexist() {
        let s = CanFrame::new(CanId::new_standard(0x7FF).unwrap(), &[1]).unwrap();
        let e = CanFrame::new(CanId::new_extended(0x1FFF_FFFF).unwrap(), &[1]).unwrap();
        assert!(s.id.is_standard());
        assert!(e.id.is_extended());
    }
}
