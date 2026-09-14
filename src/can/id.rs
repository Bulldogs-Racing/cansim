//! CAN identifier: 11-bit standard or 29-bit extended (PROMPT.md §6).
//!
//! Validation is enforced at construction so invalid states are
//! unrepresentable: [`CanId::new_standard`] rejects values above `0x7FF`,
//! [`CanId::new_extended`] rejects values above `0x1FFF_FFFF`.

use serde::{Deserialize, Serialize};
use std::fmt;
use thiserror::Error;

/// Largest valid 11-bit standard identifier.
pub const MAX_STANDARD_ID: u16 = 0x7FF;
/// Largest valid 29-bit extended identifier.
pub const MAX_EXTENDED_ID: u32 = 0x1FFF_FFFF;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CanIdError {
    #[error("standard CAN ID {0:#X} out of range (must be 0x000..=0x7FF)")]
    StandardOutOfRange(u16),
    #[error("extended CAN ID {0:#X} out of range (must be 0x00000000..=0x1FFFFFFF)")]
    ExtendedOutOfRange(u32),
}

/// Strongly-typed CAN identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CanId {
    /// 11-bit standard identifier (`0x000..=0x7FF`).
    Standard(u16),
    /// 29-bit extended identifier (`0x00000000..=0x1FFFFFFF`).
    Extended(u32),
}

impl CanId {
    /// Construct a standard 11-bit ID, validating the range.
    pub fn new_standard(raw: u16) -> Result<Self, CanIdError> {
        if raw > MAX_STANDARD_ID {
            return Err(CanIdError::StandardOutOfRange(raw));
        }
        Ok(CanId::Standard(raw))
    }

    /// Construct an extended 29-bit ID, validating the range.
    pub fn new_extended(raw: u32) -> Result<Self, CanIdError> {
        if raw > MAX_EXTENDED_ID {
            return Err(CanIdError::ExtendedOutOfRange(raw));
        }
        Ok(CanId::Extended(raw))
    }

    /// Numeric value of the identifier.
    pub fn raw(self) -> u32 {
        match self {
            CanId::Standard(v) => v as u32,
            CanId::Extended(v) => v,
        }
    }

    pub fn is_standard(self) -> bool {
        matches!(self, CanId::Standard(_))
    }

    pub fn is_extended(self) -> bool {
        matches!(self, CanId::Extended(_))
    }

    /// 11-bit base identifier used as the first arbitration field.
    ///
    /// For standard IDs this is the ID itself; for extended IDs it is the
    /// top 11 bits (`raw >> 18`), per ISO 11898-1 base/extension split.
    pub fn base_id(self) -> u16 {
        match self {
            CanId::Standard(v) => v,
            CanId::Extended(v) => ((v >> 18) & 0x7FF) as u16,
        }
    }

    /// Lower 18 bits of an extended identifier; 0 for standard IDs.
    pub fn extension_id(self) -> u32 {
        match self {
            CanId::Standard(_) => 0,
            CanId::Extended(v) => v & 0x3FFFF,
        }
    }
}

impl fmt::Display for CanId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CanId::Standard(v) => write!(f, "{:#X}", v),
            CanId::Extended(v) => write!(f, "{:#X}", v),
        }
    }
}

impl TryFrom<u16> for CanId {
    type Error = CanIdError;
    fn try_from(v: u16) -> Result<Self, Self::Error> {
        CanId::new_standard(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_range_is_enforced() {
        assert_eq!(CanId::new_standard(0x000), Ok(CanId::Standard(0)));
        assert_eq!(CanId::new_standard(0x7FF), Ok(CanId::Standard(0x7FF)));
        assert_eq!(
            CanId::new_standard(0x800),
            Err(CanIdError::StandardOutOfRange(0x800))
        );
    }

    #[test]
    fn extended_range_is_enforced() {
        assert_eq!(CanId::new_extended(0), Ok(CanId::Extended(0)));
        assert_eq!(
            CanId::new_extended(0x1FFF_FFFF),
            Ok(CanId::Extended(0x1FFF_FFFF))
        );
        assert_eq!(
            CanId::new_extended(0x2000_0000),
            Err(CanIdError::ExtendedOutOfRange(0x2000_0000))
        );
    }

    #[test]
    fn base_extension_split() {
        // 29-bit value: base = top 11 bits, ext = low 18 bits.
        let id = CanId::new_extended(0x1ABCDEFF & MAX_EXTENDED_ID).unwrap();
        assert_eq!(id.base_id(), (0x1ABCDEFFu32 >> 18) as u16 & 0x7FF);
        assert_eq!(id.extension_id(), 0x1ABCDEFF & 0x3FFFF);
        assert_eq!(CanId::Standard(0x123).base_id(), 0x123);
    }
}
