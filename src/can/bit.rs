//! Single-wire bit primitive shared by the §10–§11 bit model.
//!
//! CAN encodes dominant as `0` and recessive as `1`. Using a dedicated
//! [`Bit`] enum (instead of bare `bool`) keeps polarity explicit at every
//! call site — stuffing, CRC, and field codecs all speak the same type.

use serde::{Deserialize, Serialize};
use std::fmt;

/// One CAN bit level on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Bit {
    /// Dominant bus level, logical `0`.
    Dominant,
    /// Recessive bus level, logical `1`.
    Recessive,
}

impl Bit {
    /// Logical value: dominant = `false`, recessive = `true`.
    pub const fn as_bool(self) -> bool {
        match self {
            Bit::Dominant => false,
            Bit::Recessive => true,
        }
    }

    pub const fn is_dominant(self) -> bool {
        matches!(self, Bit::Dominant)
    }

    pub const fn is_recessive(self) -> bool {
        matches!(self, Bit::Recessive)
    }

    /// Logical negation (dominant ↔ recessive).
    pub const fn flipped(self) -> Bit {
        match self {
            Bit::Dominant => Bit::Recessive,
            Bit::Recessive => Bit::Dominant,
        }
    }
}

impl From<bool> for Bit {
    /// `false` → dominant (`0`), `true` → recessive (`1`).
    fn from(b: bool) -> Bit {
        if b {
            Bit::Recessive
        } else {
            Bit::Dominant
        }
    }
}

impl From<Bit> for bool {
    fn from(bit: Bit) -> bool {
        bit.as_bool()
    }
}

impl fmt::Display for Bit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Bit::Dominant => "0",
            Bit::Recessive => "1",
        })
    }
}

/// Append `width` bits of `value`, most-significant bit first (CAN transmits
/// multi-bit fields MSB-first).
pub fn push_value_msb(bits: &mut Vec<Bit>, value: u32, width: usize) {
    assert!(width <= 32, "field width {width} exceeds u32");
    assert!(
        width == 32 || value < (1u64 << width) as u32,
        "value {value:#X} does not fit in {width} bits"
    );
    for i in (0..width).rev() {
        bits.push(Bit::from(((value >> i) & 1) == 1));
    }
}

/// Read `width` MSB-first bits as an unsigned value.
pub fn read_value_msb(bits: &[Bit], width: usize) -> u32 {
    assert!(bits.len() >= width, "short bit slice");
    let mut value = 0u32;
    for bit in &bits[..width] {
        value = (value << 1) | (bit.as_bool() as u32);
    }
    value
}

/// Compact `"0101…"` rendering for logs and test failures.
pub fn to_bit_string(bits: &[Bit]) -> String {
    bits.iter().map(|b| char::from(*b)).collect()
}

impl From<Bit> for char {
    fn from(bit: Bit) -> char {
        if bit.is_dominant() {
            '0'
        } else {
            '1'
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn polarity_conventions() {
        assert!(!Bit::Dominant.as_bool());
        assert!(Bit::Recessive.as_bool());
        assert_eq!(Bit::from(false), Bit::Dominant);
        assert_eq!(Bit::from(true), Bit::Recessive);
        assert_eq!(Bit::Dominant.flipped(), Bit::Recessive);
    }

    #[test]
    fn msb_field_round_trip() {
        let mut bits = Vec::new();
        push_value_msb(&mut bits, 0x123, 11);
        assert_eq!(to_bit_string(&bits), "00100100011");
        assert_eq!(read_value_msb(&bits, 11), 0x123);
    }
}
