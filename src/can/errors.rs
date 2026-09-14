//! CAN error detection: kinds and error frames (PROMPT.md §12).
//!
//! Five detection kinds exist on Classical CAN: bit, stuff, CRC, form, and
//! ACK errors. A node that detects one signals an *error frame* — an error
//! flag (6 dominant bits when error-active, 6 recessive bits when
//! error-passive) followed by an 8-recessive-bit error delimiter — and all
//! nodes discard the frame. Fault-confinement accounting for these lives in
//! [`Confinement`](super::state::Confinement).
//!
//! References: ISO 11898-1; Bosch CAN Specification 2.0.

use super::bit::Bit;
use super::bits::DecodeError;
use serde::{Deserialize, Serialize};
use std::fmt;

/// The five Classical CAN error-detection kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CanErrorKind {
    /// Transmitted bit read back at the opposite level.
    Bit,
    /// Six consecutive equal bits inside the stuffing region.
    Stuff,
    /// Received CRC sequence differs from the computed one.
    Crc,
    /// Fixed-form field violation (SOF, CRC/ACK delimiters, EOF, ...).
    Form,
    /// Transmitter saw no dominant ACK slot (nobody acknowledged).
    Ack,
}

impl fmt::Display for CanErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CanErrorKind::Bit => "bit error",
            CanErrorKind::Stuff => "stuff error",
            CanErrorKind::Crc => "CRC error",
            CanErrorKind::Form => "form error",
            CanErrorKind::Ack => "ACK error",
        })
    }
}

/// Classify a codec-level decode failure into a bus-level error kind.
pub fn kind_of_decode_error(err: &DecodeError) -> CanErrorKind {
    match err {
        DecodeError::Stuff { .. } => CanErrorKind::Stuff,
        DecodeError::Crc { .. } => CanErrorKind::Crc,
        DecodeError::Form(_) | DecodeError::Truncated(_) => CanErrorKind::Form,
    }
}

/// An error frame as signalled on the bus: error flag + 8-recessive-bit
/// delimiter. Active flags (6 dominant bits) destroy the ongoing frame so
/// every node sees the error; passive flags (6 recessive) are contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorFrame {
    /// `true` for an error-active sender (dominant flag).
    pub active: bool,
}

/// Error-flag length in bits.
pub const ERROR_FLAG_LEN: usize = 6;
/// Error-delimiter length in recessive bits.
pub const ERROR_DELIMITER_LEN: usize = 8;

impl ErrorFrame {
    pub fn active() -> Self {
        ErrorFrame { active: true }
    }

    pub fn passive() -> Self {
        ErrorFrame { active: false }
    }

    /// Flag bits as driven: six dominant (active) or six recessive (passive).
    pub fn flag_bits(&self) -> Vec<Bit> {
        vec![
            if self.active {
                Bit::Dominant
            } else {
                Bit::Recessive
            };
            ERROR_FLAG_LEN
        ]
    }

    /// Full error frame bits (flag + delimiter).
    pub fn wire_bits(&self) -> Vec<Bit> {
        let mut bits = self.flag_bits();
        bits.extend(std::iter::repeat_n(Bit::Recessive, ERROR_DELIMITER_LEN));
        bits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_flag_polarity() {
        assert_eq!(ErrorFrame::active().flag_bits(), vec![Bit::Dominant; 6]);
        assert_eq!(ErrorFrame::passive().flag_bits(), vec![Bit::Recessive; 6]);
        assert_eq!(ErrorFrame::active().wire_bits().len(), 6 + 8);
    }

    #[test]
    fn decode_failures_classify() {
        assert_eq!(
            kind_of_decode_error(&DecodeError::Form("x")),
            CanErrorKind::Form
        );
        assert_eq!(
            kind_of_decode_error(&DecodeError::Truncated("x")),
            CanErrorKind::Form
        );
        assert_eq!(
            kind_of_decode_error(&DecodeError::Crc {
                expected: 1,
                actual: 2
            }),
            CanErrorKind::Crc
        );
    }
}
