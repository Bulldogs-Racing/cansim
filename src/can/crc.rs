//! Classical CAN CRC-15 (PROMPT.md §11).
//!
//! - Generator polynomial: `x^15 + x^14 + x^10 + x^8 + x^7 + x^4 + x^3 + 1`
//!   (implementation constant [`CRC_POLY`]` = 0x4599`, implicit `x^15` omitted).
//! - Shift register initialised to zero; computed over the **destuffed**
//!   protected stream `SOF … end of data field`; transmitted MSB-first.
//! - Hamming distance 6 for fixed-length frames (five random bit errors
//!   detectable, burst errors ≤ 15 bits detectable); variable stuff-bit
//!   counts reduce this for some patterns — any single-bit error is always
//!   detected (CiA, "Cyclic redundancy check in CAN frames").
//!
//! References: ISO 11898-1; Bosch CAN Specification 2.0; CiA CRC article.

use super::bit::Bit;

/// Generator polynomial without the implicit `x^15` term.
pub const CRC_POLY: u16 = 0x4599;
/// Width of the CRC sequence in bits.
pub const CRC_LEN: usize = 15;
/// Mask retaining the 15 register bits.
pub const CRC_MASK: u16 = 0x7FFF;

/// Compute the Classical CAN CRC-15 over a destuffed protected bit stream
/// (`SOF` through the last data bit), MSB-first.
///
/// Bit-serial shift-register formulation: for each input bit, the feedback
/// is `input ^ top_register_bit`; the register shifts left and XORs the
/// polynomial when feedback is set.
pub fn crc15(protected: &[Bit]) -> u16 {
    let mut crc: u16 = 0;
    for bit in protected {
        let feedback = (bit.as_bool() as u16) ^ ((crc >> 14) & 1);
        crc = (crc << 1) & CRC_MASK;
        if feedback == 1 {
            crc ^= CRC_POLY;
        }
    }
    crc
}

/// Render a 15-bit CRC value MSB-first, ready to append after the data field.
pub fn crc_to_bits(crc: u16) -> Vec<Bit> {
    assert!(crc <= CRC_MASK, "CRC value {crc:#X} exceeds 15 bits");
    (0..CRC_LEN)
        .rev()
        .map(|i| Bit::from(((crc >> i) & 1) == 1))
        .collect()
}

/// Verify a received CRC: recompute over `protected` and compare against the
/// received 15 MSB-first bits. Returns `false` (never panics) on length
/// mismatch so decoders can report [`CrcMismatch`](super::errors::DecodeError::Crc)
/// instead of crashing on truncated input.
pub fn crc_matches(protected: &[Bit], received_msb_first: &[Bit]) -> bool {
    if received_msb_first.len() != CRC_LEN {
        return false;
    }
    crc_to_bits(crc15(protected)) == received_msb_first
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::can::bit::{push_value_msb, Bit::*};

    fn bits(s: &str) -> Vec<Bit> {
        s.chars()
            .map(|c| match c {
                '0' => Dominant,
                '1' => Recessive,
                _ => panic!("bad bit char {c:?}"),
            })
            .collect()
    }

    #[test]
    fn all_zero_stream_has_zero_crc() {
        assert_eq!(crc15(&bits("000000000000000")), 0x0000);
        assert_eq!(crc15(&[]), 0x0000);
    }

    #[test]
    fn single_recessive_bit_feeds_polynomial_once() {
        // Register starts at 0, so one `1` bit yields the polynomial itself.
        assert_eq!(crc15(&bits("1")), CRC_POLY);
        assert_eq!(crc15(&bits("0")), 0x0000);
    }

    #[test]
    fn catalogue_check_value_for_123456789() {
        // CRC-15/CAN (poly 0x4599, init 0x0000, refin/refout false,
        // xorout 0x0000) check value for ASCII "123456789" is 0x059E.
        let mut stream = Vec::new();
        for byte in b"123456789" {
            push_value_msb(&mut stream, *byte as u32, 8);
        }
        assert_eq!(crc15(&stream), 0x059E);
    }

    #[test]
    fn matches_independent_crc_crate_implementation() {
        // Cross-validate against the widely-used `crc` crate's CRC_15_CAN
        // over byte-aligned streams packed MSB-first.
        let engine = crc::Crc::<u16>::new(&crc::CRC_15_CAN);
        let patterns: &[&[u8]] = &[
            b"",
            b"\x00",
            b"\xff",
            b"\x01\x02\x03\x04\x05\x06\x07\x08",
            b"123456789",
            b"\xaa\x55\xaa\x55",
            &[0x12, 0x34, 0xC0, 0xFF, 0x00, 0x7F, 0x80, 0x01],
        ];
        for payload in patterns {
            let mut stream = Vec::new();
            for byte in *payload {
                push_value_msb(&mut stream, *byte as u32, 8);
            }
            let mut digest = engine.digest();
            digest.update(payload);
            assert_eq!(
                crc15(&stream),
                digest.finalize(),
                "mismatch for payload {payload:02X?}"
            );
        }
    }

    #[test]
    fn residual_is_zero_after_appending_crc() {
        // Feeding message + its own CRC back through the register must
        // leave a zero remainder (catches Tx/Rx asymmetry bugs).
        let stream = bits("010010001110101011001100010101");
        let mut full = stream.clone();
        full.extend(crc_to_bits(crc15(&stream)));
        assert_eq!(crc15(&full), 0x0000);
    }

    #[test]
    fn every_single_bit_flip_is_detected() {
        let stream = bits("0100100011101010110011000101011111000011");
        let good = crc15(&stream);
        for i in 0..stream.len() {
            let mut bad = stream.clone();
            bad[i] = bad[i].flipped();
            assert_ne!(crc15(&bad), good, "flip at bit {i} undetected");
        }
    }

    #[test]
    fn crc_matches_compares_received_bits() {
        let protected = bits("0100100011");
        let good = crc_to_bits(crc15(&protected));
        assert!(crc_matches(&protected, &good));
        let mut bad = good.clone();
        bad[0] = bad[0].flipped();
        assert!(!crc_matches(&protected, &bad));
        assert!(!crc_matches(&protected, &good[..10]));
    }
}
