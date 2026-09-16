//! PCAP import for replay (PROMPT.md §47/§54, trace-integration slice).
//!
//! Reads `DLT_CAN_SOCKETCAN` (227) captures — the same link type the GUI's
//! PCAP export writes — back into replay scripts. Detection is by magic
//! bytes, not file extension. Every rejection names the packet index;
//! nothing is silently skipped.
//!
//! Scope (explicit): little-endian classic pcap only (what the exporter
//! writes); big-endian files, other link types, kernel error frames
//! (`CAN_ERR_FLAG`), and DLC > 8 are hard errors, not guesses.

use crate::can::frame::CanFrame;
use crate::can::id::CanId;
use thiserror::Error;

/// One bus-traffic packet from a SocketCAN capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PcapFrame {
    /// Microseconds since capture start (from the packet timestamp).
    pub time_us: u64,
    pub frame: CanFrame,
}

/// Actionable import errors (§68): every rejection names the cause.
#[derive(Debug, Error)]
pub enum PcapError {
    #[error("not a pcap file: bad magic {magic:#010X} (expected little-endian 0xA1B2C3D4)")]
    BadMagic { magic: u32 },
    #[error("unsupported pcap version {major}.{minor} (expected 2.4)")]
    BadVersion { major: u16, minor: u16 },
    #[error("unsupported link type {linktype} (only DLT_CAN_SOCKETCAN/227 imports)")]
    BadLinkType { linktype: u32 },
    #[error("packet {index}: truncated file (expected {need} bytes, got {got})")]
    Truncated {
        index: usize,
        need: usize,
        got: usize,
    },
    #[error("packet {index}: bad length {len} (SocketCAN frames are 16 bytes)")]
    BadLength { index: usize, len: u32 },
    #[error("packet {index}: kernel error frame (CAN_ERR_FLAG) is not bus traffic — strip it (e.g. editcap) and re-import")]
    KernelErrorFrame { index: usize },
    #[error("packet {index}: DLC {dlc} exceeds Classical CAN's 8 bytes")]
    BadDlc { index: usize, dlc: u8 },
    #[error("packet {index}: {cause}")]
    BadFrame { index: usize, cause: String },
}

const MAGIC_LE: u32 = 0xA1B2C3D4;
const DLT_CAN_SOCKETCAN: u32 = 227;
const CAN_EFF_FLAG: u32 = 0x8000_0000;
const CAN_RTR_FLAG: u32 = 0x4000_0000;
const CAN_ERR_FLAG: u32 = 0x2000_0000;
const CAN_FRAME_LEN: usize = 16;

/// True when `bytes` start with the little-endian pcap magic.
pub fn looks_like_pcap(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && u32::from_le_bytes(bytes[0..4].try_into().expect("len checked")) == MAGIC_LE
}

/// Parse a whole capture into timestamped frames in file order.
pub fn parse_socketcan_pcap(bytes: &[u8]) -> Result<Vec<PcapFrame>, PcapError> {
    let need = |index: usize, at: usize, len: usize| -> Result<(), PcapError> {
        if bytes.len() < at + len {
            Err(PcapError::Truncated {
                index,
                need: at + len,
                got: bytes.len(),
            })
        } else {
            Ok(())
        }
    };
    need(0, 0, 24)?;
    let magic = u32::from_le_bytes(bytes[0..4].try_into().expect("len checked"));
    if magic != MAGIC_LE {
        return Err(PcapError::BadMagic { magic });
    }
    let major = u16::from_le_bytes(bytes[4..6].try_into().expect("len checked"));
    let minor = u16::from_le_bytes(bytes[6..8].try_into().expect("len checked"));
    if (major, minor) != (2, 4) {
        return Err(PcapError::BadVersion { major, minor });
    }
    let linktype = u32::from_le_bytes(bytes[20..24].try_into().expect("len checked"));
    if linktype != DLT_CAN_SOCKETCAN {
        return Err(PcapError::BadLinkType { linktype });
    }
    let mut out = Vec::new();
    let mut at = 24;
    let mut index = 0;
    while at < bytes.len() {
        need(index, at, 16)?;
        let sec = u32::from_le_bytes(bytes[at..at + 4].try_into().expect("len checked")) as u64;
        let usec =
            u32::from_le_bytes(bytes[at + 4..at + 8].try_into().expect("len checked")) as u64;
        let incl = u32::from_le_bytes(bytes[at + 8..at + 12].try_into().expect("len checked"));
        if incl as usize != CAN_FRAME_LEN {
            return Err(PcapError::BadLength { index, len: incl });
        }
        need(index, at, 16 + CAN_FRAME_LEN)?;
        let raw = u32::from_le_bytes(bytes[at + 16..at + 20].try_into().expect("len checked"));
        if raw & CAN_ERR_FLAG != 0 {
            return Err(PcapError::KernelErrorFrame { index });
        }
        let extended = raw & CAN_EFF_FLAG != 0;
        let remote = raw & CAN_RTR_FLAG != 0;
        let id_raw = raw & 0x1FFF_FFFF;
        let dlc = bytes[at + 20];
        if dlc > 8 {
            return Err(PcapError::BadDlc { index, dlc });
        }
        let payload = &bytes[at + 24..at + 24 + 8];
        let id = if extended {
            CanId::new_extended(id_raw)
        } else if id_raw <= 0x7FF {
            CanId::new_standard(id_raw as u16)
        } else {
            // Standard range exceeded without EFF: malformed capture.
            return Err(PcapError::BadFrame {
                index,
                cause: format!("standard id {id_raw:#X} out of range without EFF flag"),
            });
        }
        .map_err(|e| PcapError::BadFrame {
            index,
            cause: e.to_string(),
        })?;
        let frame = if remote {
            CanFrame::new_remote(id, dlc)
        } else {
            CanFrame::new(id, &payload[..dlc as usize])
        }
        .map_err(|e| PcapError::BadFrame {
            index,
            cause: e.to_string(),
        })?;
        out.push(PcapFrame {
            time_us: sec * 1_000_000 + usec,
            frame,
        });
        at += 16 + CAN_FRAME_LEN;
        index += 1;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(linktype: u32) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend(MAGIC_LE.to_le_bytes());
        v.extend(2u16.to_le_bytes());
        v.extend(4u16.to_le_bytes());
        v.extend(0u32.to_le_bytes()); // thiszone
        v.extend(0u32.to_le_bytes()); // sigfigs
        v.extend(72u32.to_le_bytes()); // snaplen
        v.extend(linktype.to_le_bytes());
        v
    }

    fn packet(sec: u32, can_id: u32, dlc: u8, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend(sec.to_le_bytes());
        v.extend(500u32.to_le_bytes());
        v.extend(16u32.to_le_bytes()); // incl
        v.extend(16u32.to_le_bytes()); // orig
        v.extend(can_id.to_le_bytes());
        v.push(dlc);
        v.extend([0u8; 3]); // pad
        let mut payload = [0u8; 8];
        payload[..data.len().min(8)].copy_from_slice(&data[..data.len().min(8)]);
        v.extend(payload);
        v
    }

    #[test]
    fn parses_std_ext_and_rtr_frames() {
        let mut blob = header(DLT_CAN_SOCKETCAN);
        blob.extend(packet(1, 0x123, 2, &[0xAA, 0xBB]));
        blob.extend(packet(
            2,
            0x1ABCDE | CAN_EFF_FLAG,
            8,
            &[1, 2, 3, 4, 5, 6, 7, 8],
        ));
        blob.extend(packet(3, 0x200 | CAN_RTR_FLAG, 4, &[]));
        let frames = parse_socketcan_pcap(&blob).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].time_us, 1_000_500);
        assert_eq!(format!("{}", frames[0].frame.id), "0x123");
        assert_eq!(frames[0].frame.data, vec![0xAA, 0xBB]);
        assert!(matches!(frames[1].frame.id, CanId::Extended(0x1ABCDE)));
        assert!(frames[2].frame.is_remote);
        assert_eq!(frames[2].frame.dlc, 4);
        assert!(!looks_like_pcap(b"{}"));
        assert!(looks_like_pcap(&blob));
    }

    #[test]
    fn rejects_everything_explicitly() {
        assert!(matches!(
            parse_socketcan_pcap(b"nope").unwrap_err(),
            PcapError::Truncated { .. }
        ));
        let mut bad_magic = vec![0u8; 24];
        bad_magic[0..4].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        assert!(matches!(
            parse_socketcan_pcap(&bad_magic).unwrap_err(),
            PcapError::BadMagic { .. }
        ));
        assert!(matches!(
            parse_socketcan_pcap(&header(1)).unwrap_err(),
            PcapError::BadLinkType { .. }
        ));
        let mut v = header(DLT_CAN_SOCKETCAN);
        let mut short = packet(0, 0x100, 2, &[1, 2]);
        short.truncate(20);
        v.extend(short);
        assert!(matches!(
            parse_socketcan_pcap(&v).unwrap_err(),
            PcapError::Truncated { .. }
        ));
        let mut v = header(DLT_CAN_SOCKETCAN);
        let mut bad_len = packet(0, 0x100, 2, &[1, 2]);
        bad_len[8..12].copy_from_slice(&8u32.to_le_bytes());
        v.extend(bad_len);
        assert!(matches!(
            parse_socketcan_pcap(&v).unwrap_err(),
            PcapError::BadLength { .. }
        ));
        let mut v = header(DLT_CAN_SOCKETCAN);
        v.extend(packet(0, 0x100 | CAN_ERR_FLAG, 2, &[1, 2]));
        assert!(matches!(
            parse_socketcan_pcap(&v).unwrap_err(),
            PcapError::KernelErrorFrame { .. }
        ));
        let mut v = header(DLT_CAN_SOCKETCAN);
        v.extend(packet(0, 0x100, 9, &[]));
        assert!(matches!(
            parse_socketcan_pcap(&v).unwrap_err(),
            PcapError::BadDlc { .. }
        ));
        let mut v = header(DLT_CAN_SOCKETCAN);
        v.extend(packet(0, 0x800, 0, &[]));
        assert!(matches!(
            parse_socketcan_pcap(&v).unwrap_err(),
            PcapError::BadFrame { .. }
        ));
    }
}
