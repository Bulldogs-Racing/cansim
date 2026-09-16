//! Bit-level Classical CAN frame codec: `SOF … EOF` (PROMPT.md §10).
//!
//! Field layout in transmission order (ISO 11898-1 / Bosch CAN 2.0A/B):
//!
//! ```text
//! Standard: SOF IDE──┐
//!           SOF(0) ID(11) RTR IDE(0) r0(0) DLC(4) DATA(0–64) CRC(15) CRCd ACK·ACKd EOF(7)
//! Extended: SOF BASE(11) SRR(1) IDE(1) EXT(18) RTR r1(0) r0(0) DLC(4) DATA CRC CRCd ACK·ACKd EOF
//! ```
//!
//! - Bit stuffing covers `SOF` through the CRC sequence; the CRC delimiter,
//!   ACK field, EOF (and IFS, which is bus idle rather than frame content)
//!   are fixed-form and never stuffed.
//! - The CRC is computed over the destuffed `SOF … end-of-data` stream.
//! - The transmitted ACK slot is recessive; receivers overwrite it with a
//!   dominant bit. Decoders therefore accept either level in the ACK slot
//!   and report what they saw ([`DecodeInfo::acked`]).
//! - Raw DLC values 9–15 denote 8 data bytes on real hardware (ISO 11898-1
//!   DLC table); the decoder maps them, while the encoder only ever emits
//!   0–8 (see [`CanFrame`]).
//! - Reserved bits (`r0`/`r1`) and `SRR` are accepted at either level:
//!   real receivers ignore them, so rejecting frames on those bits would
//!   invent discards the protocol does not define.

use super::bit::{push_value_msb, read_value_msb, Bit};
use super::crc::{crc15, crc_to_bits, CRC_LEN};
use super::frame::CanFrame;
use super::id::CanId;
use super::stuffing::{StuffError, STUFF_RUN};
use thiserror::Error;

/// Fixed-form tail: CRC delimiter (recessive, not stuffed).
pub const TAIL_CRC_DELIM: Bit = Bit::Recessive;
/// EOF length in recessive bits.
pub const EOF_LEN: usize = 7;
/// Inter-frame space in recessive bits (bus idle between frames; idle
/// nodes must also observe it, but it is timing, not frame content, so
/// [`encode_frame`] excludes it while [`wire_duration_ns`] includes it).
pub const IFS_LEN: usize = 3;

/// A frame encoded to wire bits, with each layer exposed for inspection
/// (analyzer waveform views and tests consume these separately, §33).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedFrame {
    /// Destuffed protected stream `SOF … end of data` (CRC input).
    pub protected: Vec<Bit>,
    /// CRC-15 value over [`EncodedFrame::protected`].
    pub crc: u16,
    /// Stuffed `protected + CRC bits`.
    pub stuffed: Vec<Bit>,
    /// Complete frame `SOF … EOF` as driven by the transmitter
    /// (ACK slot recessive; receivers overwrite it — see bus layer).
    pub wire: Vec<Bit>,
    /// Number of stuff bits inside [`EncodedFrame::stuffed`].
    pub stuff_bits: usize,
}

/// What the decoder observed beyond the logical frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodeInfo {
    /// Stuff bits consumed while decoding.
    pub stuff_bits: usize,
    /// CRC-15 value read from the wire.
    pub crc: u16,
    /// `true` when the ACK slot was dominant (some node acknowledged).
    pub acked: bool,
    /// Raw 4-bit DLC nibble (9–15 map to 8 data bytes).
    pub dlc_raw: u8,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DecodeError {
    #[error("frame truncated: {0}")]
    Truncated(&'static str),
    #[error("form error: {0}")]
    Form(&'static str),
    #[error("stuff error at wire offset {offset}: {source}")]
    Stuff {
        offset: usize,
        #[source]
        source: StuffError,
    },
    #[error("CRC mismatch: wire has {actual:#06X}, computed {expected:#06X}")]
    Crc { expected: u16, actual: u16 },
}

/// Encode a logical frame to wire bits `SOF … EOF`.
pub fn encode_frame(frame: &CanFrame) -> EncodedFrame {
    let mut protected = Vec::new();
    protected.push(Bit::Dominant); // SOF
    match frame.id {
        CanId::Standard(id) => {
            push_value_msb(&mut protected, id as u32, 11);
            protected.push(Bit::from(frame.is_remote)); // RTR
            protected.push(Bit::Dominant); // IDE = standard
            protected.push(Bit::Dominant); // r0
        }
        CanId::Extended(_) => {
            push_value_msb(&mut protected, frame.id.base_id() as u32, 11);
            protected.push(Bit::Recessive); // SRR
            protected.push(Bit::Recessive); // IDE = extended
            push_value_msb(&mut protected, frame.id.extension_id(), 18);
            protected.push(Bit::from(frame.is_remote)); // RTR
            protected.push(Bit::Dominant); // r1
            protected.push(Bit::Dominant); // r0
        }
    }
    push_value_msb(&mut protected, frame.dlc as u32, 4);
    if !frame.is_remote {
        for byte in &frame.data {
            push_value_msb(&mut protected, *byte as u32, 8);
        }
    }

    let crc = crc15(&protected);
    let mut unstuffed_tail = protected.clone();
    unstuffed_tail.extend(crc_to_bits(crc));
    let stuffed = crate::can::stuffing::stuff(&unstuffed_tail);
    let stuff_bits = stuffed.len() - unstuffed_tail.len();

    let mut wire = stuffed.clone();
    wire.push(TAIL_CRC_DELIM);
    wire.push(Bit::Recessive); // ACK slot as transmitted
    wire.push(Bit::Recessive); // ACK delimiter
    wire.extend(std::iter::repeat_n(Bit::Recessive, EOF_LEN));

    EncodedFrame {
        protected,
        crc,
        stuffed,
        wire,
        stuff_bits,
    }
}

/// Nominal wire length `SOF … EOF` in bits (stuffing-exact for this frame).
pub fn wire_len(frame: &CanFrame) -> usize {
    encode_frame(frame).wire.len()
}

/// Transmission time in nanoseconds at `bitrate_bit_s`, counting the exact
/// stuffed frame bits plus the inter-frame space.
pub fn wire_duration_ns(frame: &CanFrame, bitrate_bit_s: u32) -> u64 {
    assert!(bitrate_bit_s > 0, "bitrate must be positive");
    (encode_frame(frame).wire.len() + IFS_LEN) as u64 * 1_000_000_000 / bitrate_bit_s as u64
}

/// Incremental destuffing reader: yields destuffed bits from a stuffed
/// region while tracking the consumed wire offset.
struct DestuffReader<'a> {
    wire: &'a [Bit],
    /// Next unread wire offset.
    pos: usize,
    /// Current run of equal destuffed bits (stuff bits reset it).
    run: usize,
    last: Option<Bit>,
    pub stuff_bits: usize,
}

impl<'a> DestuffReader<'a> {
    fn new(wire: &'a [Bit]) -> Self {
        DestuffReader {
            wire,
            pos: 0,
            run: 0,
            last: None,
            stuff_bits: 0,
        }
    }

    /// Read one destuffed bit, consuming any following stuff bit.
    fn next(&mut self) -> Result<Bit, DecodeError> {
        let bit = *self.wire.get(self.pos).ok_or(DecodeError::Truncated(
            "unexpected end inside stuffed region",
        ))?;
        self.pos += 1;
        if Some(bit) == self.last {
            self.run += 1;
        } else {
            self.run = 1;
            self.last = Some(bit);
        }
        if self.run == STUFF_RUN + 1 {
            return Err(DecodeError::Stuff {
                offset: self.pos - 1,
                source: StuffError::TooManyConsecutive {
                    run: self.run,
                    polarity: if bit.is_dominant() {
                        "dominant"
                    } else {
                        "recessive"
                    },
                    offset: self.pos - 1,
                },
            });
        }
        if self.run == STUFF_RUN {
            match self.wire.get(self.pos) {
                None => return Err(DecodeError::Truncated("run of five at end of input")),
                Some(next) if *next == bit => {
                    return Err(DecodeError::Stuff {
                        offset: self.pos,
                        source: StuffError::TooManyConsecutive {
                            run: STUFF_RUN + 1,
                            polarity: if bit.is_dominant() {
                                "dominant"
                            } else {
                                "recessive"
                            },
                            offset: self.pos,
                        },
                    });
                }
                Some(_) => {
                    self.pos += 1; // consume stuff bit
                    self.stuff_bits += 1;
                    self.last = Some(bit.flipped());
                    self.run = 1;
                }
            }
        }
        Ok(bit)
    }

    fn next_field(&mut self, width: usize, what: &'static str) -> Result<u32, DecodeError> {
        if width == 0 {
            return Ok(0);
        }
        let mut value = 0u32;
        for _ in 0..width {
            let bit = self.next()?;
            value = (value << 1) | (bit.as_bool() as u32);
        }
        let _ = what;
        Ok(value)
    }
}

/// Decode wire bits `SOF … EOF` (trailing IFS/idle bits are ignored).
pub fn decode_frame(wire: &[Bit]) -> Result<(CanFrame, DecodeInfo), DecodeError> {
    let mut r = DestuffReader::new(wire);

    // SOF: exactly one dominant bit.
    match r.next()? {
        Bit::Dominant => {}
        Bit::Recessive => return Err(DecodeError::Form("SOF must be dominant")),
    }
    let protected_start = 0;

    let base = r.next_field(11, "base ID")? as u16;
    let b12 = r.next()?;
    let ide = r.next()?;

    let (id, is_remote, dlc_raw) = if ide.is_recessive() {
        // Extended: b12 was SRR, then 18-bit extension, RTR, r1, r0, DLC.
        let ext = r.next_field(18, "extended ID")?;
        let rtr = r.next()?;
        let _r1 = r.next()?;
        let _r0 = r.next()?;
        let dlc = r.next_field(4, "DLC")? as u8;
        let raw = ((base as u32) << 18) | ext;
        (
            CanId::new_extended(raw).map_err(|_| DecodeError::Form("extended ID out of range"))?,
            rtr.is_recessive(),
            dlc,
        )
    } else {
        // Standard: b12 was RTR, then r0, DLC.
        let rtr = b12;
        let _r0 = r.next()?;
        let dlc = r.next_field(4, "DLC")? as u8;
        (
            CanId::new_standard(base).map_err(|_| DecodeError::Form("standard ID out of range"))?,
            rtr.is_recessive(),
            dlc,
        )
    };

    // ISO 11898-1 DLC table: nibbles 9–15 denote 8 data bytes.
    let data_len = if dlc_raw > 8 { 8 } else { dlc_raw } as usize;
    let mut data = Vec::with_capacity(if is_remote { 0 } else { data_len });
    if !is_remote {
        for _ in 0..data_len {
            data.push(r.next_field(8, "data")? as u8);
        }
    }

    // CRC sequence (still inside the stuffed region).
    let mut crc_bits = Vec::with_capacity(CRC_LEN);
    for _ in 0..CRC_LEN {
        crc_bits.push(r.next()?);
    }
    let actual = read_value_msb(&crc_bits, CRC_LEN) as u16;

    // Everything destuffed so far is the protected stream. Re-slice it from
    // the wire-independent record: rebuild by re-reading is wasteful, so
    // instead recompute the CRC over a re-encoded prefix of the decoded
    // frame? No — that would be circular (a decoder bug would hide itself).
    // Recover the true protected bits by stripping stuff bits from the
    // consumed wire prefix with the standalone primitive.
    let consumed = r.pos;
    let protected =
        crate::can::stuffing::destuff(&wire[protected_start..consumed]).map_err(|source| {
            DecodeError::Stuff {
                offset: consumed,
                source,
            }
        })?;
    // Drop the 15 CRC bits at the end to obtain SOF..data.
    let protected = &protected[..protected.len() - CRC_LEN];
    let expected = crc15(protected);
    if expected != actual {
        return Err(DecodeError::Crc { expected, actual });
    }

    // Fixed-form tail: read raw (never destuffed).
    let mut tail = r.pos;
    let mut take_raw = |what: &'static str| -> Result<Bit, DecodeError> {
        let bit = *wire.get(tail).ok_or(DecodeError::Truncated(what))?;
        tail += 1;
        Ok(bit)
    };
    if take_raw("CRC delimiter")?.is_dominant() {
        return Err(DecodeError::Form("CRC delimiter must be recessive"));
    }
    let acked = take_raw("ACK slot")?.is_dominant();
    if take_raw("ACK delimiter")?.is_dominant() {
        return Err(DecodeError::Form("ACK delimiter must be recessive"));
    }
    for _ in 0..EOF_LEN {
        if take_raw("EOF")?.is_dominant() {
            return Err(DecodeError::Form("EOF must be seven recessive bits"));
        }
    }

    let frame = if is_remote {
        CanFrame::new_remote(id, if dlc_raw > 8 { 8 } else { dlc_raw })
            .map_err(|_| DecodeError::Form("remote DLC out of range"))?
    } else {
        CanFrame::new(id, &data).map_err(|_| DecodeError::Form("payload length out of range"))?
    };
    Ok((
        frame,
        DecodeInfo {
            stuff_bits: r.stuff_bits,
            crc: actual,
            acked,
            dlc_raw,
        },
    ))
}

/// One named region of the unstuffed frame (`SOF … CRC sequence`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRegion {
    /// `SOF`, `Arbitration`, `Control`, `Data`, or `CRC`.
    pub name: &'static str,
    /// Offset in unstuffed bits from SOF.
    pub offset: usize,
    pub bits: Vec<Bit>,
}

/// Bit-level layout of a frame (Phase 10, slice 1): named unstuffed
/// regions plus the full transmitted wire `SOF … EOF` for future waveform
/// rendering. Boundaries mirror [`encode_frame`] exactly — SOF(1), then
/// arbitration (14 standard / 34 extended), control DLC(4), data, CRC(15);
/// the fixed tail (CRC delimiter, ACK slot, ACK delimiter, EOF) is not
/// stuffed and not regioned, but included in [`FrameLayout::wire`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameLayout {
    pub regions: Vec<FrameRegion>,
    pub crc: u16,
    pub stuff_bits: usize,
    pub wire: Vec<Bit>,
}

pub fn describe_frame(frame: &CanFrame) -> FrameLayout {
    let enc = encode_frame(frame);
    let arb_len = match frame.id {
        CanId::Standard(_) => 11 + 1 + 1 + 1,
        CanId::Extended(_) => 11 + 1 + 1 + 18 + 1 + 1 + 1,
    };
    let data_len = enc.protected.len() - (1 + arb_len + 4);
    let mut regions = Vec::with_capacity(5);
    let mut at = 0;
    let mut take = |name: &'static str, len: usize| {
        let bits = enc.protected[at..at + len].to_vec();
        regions.push(FrameRegion {
            name,
            offset: at,
            bits,
        });
        at += len;
    };
    take("SOF", 1);
    take("Arbitration", arb_len);
    take("Control", 4);
    take("Data", data_len);
    debug_assert_eq!(at, enc.protected.len());
    regions.push(FrameRegion {
        name: "CRC",
        offset: at,
        bits: crc_to_bits(enc.crc),
    });
    FrameLayout {
        regions,
        crc: enc.crc,
        stuff_bits: enc.stuff_bits,
        wire: enc.wire,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::can::bit::to_bit_string;
    use Bit::*;

    #[test]
    fn describe_splits_known_layouts() {
        // Region geometry on an 8-byte standard frame.
        let big = CanFrame::new(CanId::new_standard(0x123).unwrap(), &[0; 8]).unwrap();
        let layout = describe_frame(&big);
        let lens: Vec<(&str, usize)> = layout
            .regions
            .iter()
            .map(|r| (r.name, r.bits.len()))
            .collect();
        assert_eq!(
            lens,
            vec![
                ("SOF", 1),
                ("Arbitration", 14),
                ("Control", 4),
                ("Data", 64),
                ("CRC", 15),
            ]
        );
        assert_eq!(layout.regions[0].offset, 0);
        assert_eq!(layout.regions[4].offset, 1 + 14 + 4 + 64);
        // Wire consistency: stuffed protected+CRC, then the fixed tail
        // (CRC delimiter, ACK slot, ACK delimiter, EOF).
        assert_eq!(
            layout.wire.len(),
            layout.regions.iter().map(|r| r.bits.len()).sum::<usize>()
                + layout.stuff_bits
                + 1
                + 1
                + 1
                + EOF_LEN
        );
        assert_eq!(to_bit_string(&layout.wire[..1]), "0"); // SOF dominant
                                                           // The established hand-checkable vector (19 zero bits → CRC 0)
                                                           // describes to the same CRC the encoder emits.
        let zero = std_frame(0x000, &[]);
        let layout = describe_frame(&zero);
        assert_eq!(layout.crc, 0x0000);
        assert_eq!(layout.regions[3].bits.len(), 0);
        assert_eq!(layout.regions[4].offset, 1 + 14 + 4);
        // Layout CRC agrees with an independent decode of the wire.
        let (decoded, info) = decode_frame(&layout.wire).unwrap();
        assert_eq!(decoded, zero);
        assert_eq!(info.crc, layout.crc);

        // Extended frame: 34-bit arbitration; remote frame: empty data.
        let ext = CanFrame::new(CanId::new_extended(0x1ABCDE).unwrap(), &[1, 2]).unwrap();
        let layout = describe_frame(&ext);
        assert_eq!(layout.regions[1].bits.len(), 34);
        assert_eq!(layout.regions[3].bits.len(), 16);
        let rtr = CanFrame::new_remote(CanId::new_standard(0x200).unwrap(), 4).unwrap();
        let layout = describe_frame(&rtr);
        assert_eq!(layout.regions[3].bits.len(), 0);
        assert_eq!(layout.regions[4].offset, 1 + 14 + 4);
    }

    fn std_frame(id: u16, data: &[u8]) -> CanFrame {
        CanFrame::new(CanId::new_standard(id).unwrap(), data).unwrap()
    }

    #[test]
    fn all_zero_miniframe_has_zero_crc_and_known_prefix() {
        // Hand-checkable vector: SOF + 11 ID zeros + RTR(0) + IDE(0) +
        // r0(0) + DLC(0000) = 19 zero bits → CRC-15 = 0x0000.
        let frame = std_frame(0x000, &[]);
        let enc = encode_frame(&frame);
        assert_eq!(enc.crc, 0x0000);
        assert_eq!(enc.protected, vec![Dominant; 19]);
        assert_eq!(&to_bit_string(&enc.wire)[..1], "0"); // SOF dominant
        let (decoded, info) = decode_frame(&enc.wire).unwrap();
        assert_eq!(decoded, frame);
        assert!(!info.acked); // transmitted ACK slot is recessive
    }

    #[test]
    fn round_trips_all_dlc_lengths_and_formats() {
        let payload = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
        for dlc in 0..=8 {
            for frame in [
                std_frame(0x123, &payload[..dlc]),
                std_frame(0x7FF, &payload[..dlc]),
                CanFrame::new(CanId::new_extended(0x1ABCDE).unwrap(), &payload[..dlc]).unwrap(),
                CanFrame::new(CanId::new_extended(0x1FFF_FFFF).unwrap(), &payload[..dlc]).unwrap(),
                CanFrame::new_remote(CanId::new_standard(0x200).unwrap(), dlc as u8).unwrap(),
                CanFrame::new_remote(CanId::new_extended(0x12345).unwrap(), dlc as u8).unwrap(),
            ] {
                let enc = encode_frame(&frame);
                let (decoded, info) = decode_frame(&enc.wire)
                    .unwrap_or_else(|e| panic!("round-trip failed for {frame:?}: {e}"));
                assert_eq!(decoded, frame);
                assert_eq!(info.stuff_bits, enc.stuff_bits);
                assert_eq!(info.crc, enc.crc);
            }
        }
    }

    #[test]
    fn worst_case_payloads_round_trip() {
        for payload in [
            vec![0x00; 8],
            vec![0xFF; 8],
            vec![0xAA; 8],
            vec![0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF, 0x00, 0xFF],
        ] {
            let frame = std_frame(0x555, &payload);
            let enc = encode_frame(&frame);
            assert!(enc.stuff_bits > 0, "worst case should stuff");
            assert_eq!(decode_frame(&enc.wire).unwrap().0, frame);
        }
    }

    #[test]
    fn encoder_matches_independent_manual_construction() {
        // Build the same wire through the field primitives instead of
        // encode_frame, then require bit-equality (second code path).
        let frame = std_frame(0x100, &[0xA5]);
        let mut protected = vec![Dominant];
        for b in [0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0] {
            protected.push(Bit::from(b == 1));
        }
        protected.push(Dominant); // RTR = data
        protected.push(Dominant); // IDE
        protected.push(Dominant); // r0
        for b in [0, 0, 0, 1] {
            protected.push(Bit::from(b == 1));
        } // DLC = 1
        for b in [1, 0, 1, 0, 0, 1, 0, 1] {
            protected.push(Bit::from(b == 1));
        } // 0xA5
        let crc = crc15(&protected);
        let mut tail = protected.clone();
        tail.extend(crc_to_bits(crc));
        let stuffed = crate::can::stuffing::stuff(&tail);
        let mut wire = stuffed;
        wire.push(Recessive);
        wire.push(Recessive);
        wire.push(Recessive);
        wire.extend(std::iter::repeat_n(Recessive, EOF_LEN));
        assert_eq!(encode_frame(&frame).wire, wire);
    }

    #[test]
    fn every_wire_bit_flip_is_detected_except_ack_slot() {
        let frame = std_frame(0x123, &[0x01, 0x02, 0x03, 0x04]);
        let enc = encode_frame(&frame);
        let ack_slot = enc.stuffed.len() + 1; // CRC delim, then ACK slot
        assert!(enc.wire[ack_slot].is_recessive());
        for i in 0..enc.wire.len() {
            let mut bad = enc.wire.clone();
            bad[i] = bad[i].flipped();
            let result = decode_frame(&bad);
            if i == ack_slot {
                // Either ACK level is legal; the frame must still decode.
                let (decoded, info) = result.unwrap();
                assert_eq!(decoded, frame);
                assert!(info.acked);
            } else {
                assert!(result.is_err(), "flip at wire bit {i} undetected");
            }
        }
    }

    #[test]
    fn fixed_form_violations_are_form_errors() {
        let frame = std_frame(0x123, &[1, 2, 3]);
        let enc = encode_frame(&frame);
        // CRC delimiter driven dominant.
        let mut bad = enc.wire.clone();
        bad[enc.stuffed.len()] = Dominant;
        assert!(matches!(decode_frame(&bad), Err(DecodeError::Form(_))));
        // EOF bit driven dominant.
        let mut bad = enc.wire.clone();
        let eof_start = bad.len() - EOF_LEN;
        bad[eof_start + 3] = Dominant;
        assert!(matches!(decode_frame(&bad), Err(DecodeError::Form(_))));
        // Recessive SOF.
        let mut bad = enc.wire.clone();
        bad[0] = Recessive;
        assert!(matches!(decode_frame(&bad), Err(DecodeError::Form(_))));
    }

    #[test]
    fn truncation_is_reported() {
        let frame = std_frame(0x123, &[1, 2, 3, 4]);
        let enc = encode_frame(&frame);
        assert!(matches!(
            decode_frame(&enc.wire[..10]),
            Err(DecodeError::Truncated(_))
        ));
        assert!(matches!(decode_frame(&[]), Err(DecodeError::Truncated(_))));
    }

    #[test]
    fn dlc_nibbles_9_to_15_decode_as_8_bytes() {
        // Take a DLC-8 encoding, patch the protected DLC nibble to 15,
        // recompute CRC over the patched stream, re-stuff, re-tail.
        let frame = std_frame(0x200, &[9; 8]);
        let enc = encode_frame(&frame);
        let mut protected = enc.protected.clone();
        // Protected layout (std): SOF(1) ID(11) RTR IDE r0 DLC(4) DATA.
        let dlc_at = 1 + 11 + 1 + 1 + 1;
        for (k, bit) in [Recessive, Recessive, Recessive, Recessive]
            .into_iter()
            .enumerate()
        {
            protected[dlc_at + k] = bit; // DLC := 15
        }
        let crc = crc15(&protected);
        let mut tail = protected;
        tail.extend(crc_to_bits(crc));
        let mut wire = crate::can::stuffing::stuff(&tail);
        wire.push(Recessive);
        wire.push(Recessive);
        wire.push(Recessive);
        wire.extend(std::iter::repeat_n(Recessive, EOF_LEN));
        let (decoded, info) = decode_frame(&wire).unwrap();
        assert_eq!(info.dlc_raw, 15);
        assert_eq!(decoded.dlc, 8);
        assert_eq!(decoded.data, vec![9; 8]);
    }
}
