//! DBC subset: message/signal definitions plus payload decoding.
//!
//! Phase 12, slice 1 (PROMPT.md §38): import a `.dbc` file's message and
//! signal layout, then decode Classical CAN payloads into physical signal
//! values. This is a deliberately small subset, documented below —
//! everything else is an explicit error, never a silent misread.
//!
//! Parsed: `VERSION`, `BU_` node lists, `BO_ <id> <name>: <dlc> <tx>`
//! messages, and non-multiplexed `SG_` signals:
//!
//! ```text
//! SG_ WheelSpeed : 7|16@1+ (0.1,0) [0|6553.5] "km/h" ABS
//! ```
//!
//! meaning: start bit 7, 16 bits, `@1` little-endian (Intel), `+`
//! unsigned, factor 0.1, offset 0, range, unit, receivers. `@0` is
//! big-endian (Motorola, start bit = MSB with Vector sawtooth numbering).
//!
//! Deferred (explicit): multiplexed signals (`m0`/`M` markers are
//! rejected), extended multiplexing, `VAL_` value tables, `BA_`
//! attributes, `CM_` comments, signal value descriptions, J1939
//! protocol decoding, and any DBC *writing*.

use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

/// Actionable DBC errors (§68): every rejection names the file and line.
#[derive(Debug, Error)]
pub enum DbcError {
    #[error("cannot read DBC file {path}: {cause}")]
    Io { path: String, cause: String },
    #[error(
        "{path}:{line}: malformed message line (expected `BO_ <id> <name>: <dlc> <tx>`): {text}"
    )]
    BadMessage {
        path: String,
        line: usize,
        text: String,
    },
    #[error("{path}:{line}: malformed signal line (expected `SG_ <name> : <start>|<len>@<endian><sign> (<factor>,<offset>) [<min>|<max>] \"<unit>\" <rx...>`): {text}")]
    BadSignal {
        path: String,
        line: usize,
        text: String,
    },
    #[error("{path}:{line}: signal \"{signal}\" uses multiplexing (`{marker}`), which this build does not decode — flatten the multiplexer or wait for Phase 12 depth")]
    Multiplexed {
        path: String,
        line: usize,
        signal: String,
        marker: String,
    },
    #[error("{path}:{line}: signal \"{signal}\" has length {len}, but Classical CAN DBC signals are 1..=32 bits")]
    BadLength {
        path: String,
        line: usize,
        signal: String,
        len: u32,
    },
    #[error("unknown message id {id:#X} (DBC defines: {known})")]
    UnknownMessage { id: u32, known: String },
    #[error("message {name} ({id:#X}) needs {need} payload bytes, got {got}")]
    ShortPayload {
        name: String,
        id: u32,
        need: usize,
        got: usize,
    },
}

/// One `SG_` signal: bit layout plus linear scaling.
#[derive(Debug, Clone, PartialEq)]
pub struct DbcSignal {
    pub name: String,
    pub start_bit: u32,
    pub len: u32,
    /// `true` = `@1` little-endian (Intel); `false` = `@0` big-endian (Motorola).
    pub little_endian: bool,
    pub signed: bool,
    pub factor: f64,
    pub offset: f64,
    pub min: f64,
    pub max: f64,
    pub unit: String,
    pub receivers: Vec<String>,
}

/// One `BO_` message with its signals in file order.
#[derive(Debug, Clone, PartialEq)]
pub struct DbcMessage {
    pub id: u32,
    pub name: String,
    pub dlc: u8,
    pub transmitter: String,
    pub signals: Vec<DbcSignal>,
}

/// A parsed DBC file: messages keyed by id, node list kept for reference.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dbc {
    pub version: String,
    pub nodes: Vec<String>,
    pub messages: BTreeMap<u32, DbcMessage>,
}

impl Dbc {
    pub fn load_from_file(path: &Path) -> Result<Self, DbcError> {
        let text = std::fs::read_to_string(path).map_err(|e| DbcError::Io {
            path: path.display().to_string(),
            cause: e.to_string(),
        })?;
        Self::parse(&text, &path.display().to_string())
    }

    /// Parse DBC text. Unknown top-level sections (`NS_`, `BS_`, `CM_`,
    /// `BA_`, `VAL_`, …) are ignored — only message/signal geometry matters
    /// here. Malformed `BO_`/`SG_` lines and multiplexing are errors.
    pub fn parse(text: &str, origin: &str) -> Result<Self, DbcError> {
        let mut dbc = Dbc::default();
        let mut current: Option<u32> = None;
        for (idx, raw) in text.lines().enumerate() {
            let line_no = idx + 1;
            let line = raw.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(rest) = line.strip_prefix("VERSION") {
                dbc.version = rest.trim().trim_matches('"').to_string();
                current = None;
            } else if let Some(rest) = line.strip_prefix("BU_") {
                dbc.nodes = rest
                    .split_whitespace()
                    .map(|s| s.trim_end_matches(':').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                current = None;
            } else if let Some(rest) = line.strip_prefix("BO_ ") {
                let msg = parse_message(rest, origin, line_no, line)?;
                current = Some(msg.id);
                dbc.messages.insert(msg.id, msg);
            } else if let Some(rest) = line.strip_prefix("SG_ ") {
                let Some(id) = current else {
                    return Err(DbcError::BadSignal {
                        path: origin.into(),
                        line: line_no,
                        text: line.into(),
                    });
                };
                let sig = parse_signal(rest, origin, line_no, line)?;
                dbc.messages
                    .get_mut(&id)
                    .expect("current message exists")
                    .signals
                    .push(sig);
            } else {
                // Any other section (NS_, BS_, CM_, BA_, VAL_, …): out of
                // subset scope, ignored by design — but it also ends the
                // current message's signal block.
                current = None;
            }
        }
        Ok(dbc)
    }

    pub fn message(&self, id: u32) -> Result<&DbcMessage, DbcError> {
        self.messages
            .get(&id)
            .ok_or_else(|| DbcError::UnknownMessage {
                id,
                known: self
                    .messages
                    .keys()
                    .map(|k| format!("{k:#X}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            })
    }

    /// Decode one payload into `(signal, physical value, unit)` triples in
    /// signal order. Payloads shorter than the message DLC are an error;
    /// longer ones decode their leading bytes (a sender may pad).
    pub fn decode(&self, id: u32, payload: &[u8]) -> Result<Vec<(String, f64, String)>, DbcError> {
        let msg = self.message(id)?;
        if payload.len() < msg.dlc as usize {
            return Err(DbcError::ShortPayload {
                name: msg.name.clone(),
                id,
                need: msg.dlc as usize,
                got: payload.len(),
            });
        }
        let mut padded = [0u8; 8];
        padded[..payload.len().min(8)].copy_from_slice(&payload[..payload.len().min(8)]);
        Ok(msg
            .signals
            .iter()
            .map(|s| {
                let raw = extract_raw(&padded, s);
                (s.name.clone(), raw * s.factor + s.offset, s.unit.clone())
            })
            .collect())
    }
}

/// Raw (unscaled) signal value with sign extension applied.
fn extract_raw(payload: &[u8; 8], sig: &DbcSignal) -> f64 {
    let mut raw: u64 = 0;
    if sig.little_endian {
        // Intel: start bit is the LSB; consecutive bits ascend.
        for i in 0..sig.len {
            let pos = sig.start_bit + i;
            let bit = (payload[(pos / 8) as usize] >> (pos % 8)) & 1;
            raw |= (bit as u64) << i;
        }
    } else {
        // Motorola: start bit is the MSB; consecutive bits descend within
        // each byte (LSB-0 positions), then continue at the next byte's MSB
        // (sawtooth: ...8 -> 23...). Bit extraction is LSB-0 like Intel —
        // only the position sequence and MSB-first accumulation differ.
        let mut pos = sig.start_bit;
        for _ in 0..sig.len {
            let bit = (payload[(pos / 8) as usize] >> (pos % 8)) & 1;
            raw = (raw << 1) | bit as u64;
            pos = if pos.is_multiple_of(8) {
                pos + 15
            } else {
                pos - 1
            };
        }
    }
    if sig.signed {
        // Sign-extend the len-bit two's complement value.
        let shift = 64 - sig.len;
        (((raw << shift) as i64) >> shift) as f64
    } else {
        raw as f64
    }
}

fn parse_message(
    rest: &str,
    origin: &str,
    line: usize,
    text: &str,
) -> Result<DbcMessage, DbcError> {
    // `100 EngineData: 8 Vector__XXX`
    let bad = || DbcError::BadMessage {
        path: origin.into(),
        line,
        text: text.into(),
    };
    let (id_part, after_id) = rest.split_once(char::is_whitespace).ok_or_else(bad)?;
    let id: u32 = id_part.parse().map_err(|_| bad())?;
    let (name_part, after_name) = after_id.trim_start().split_once(':').ok_or_else(bad)?;
    let mut tail = after_name.split_whitespace();
    let dlc: u8 = tail.next().ok_or_else(bad)?.parse().map_err(|_| bad())?;
    let transmitter = tail.next().ok_or_else(bad)?.to_string();
    Ok(DbcMessage {
        id,
        name: name_part.trim().to_string(),
        dlc,
        transmitter,
        signals: Vec::new(),
    })
}

fn parse_signal(rest: &str, origin: &str, line: usize, text: &str) -> Result<DbcSignal, DbcError> {
    // `WheelSpeed : 7|16@1+ (0.1,0) [0|6553.5] "km/h" ABS`
    // `Temp : 0|8@1- (1,-40) [-40|215] "degC" Vector__XXX`
    // `Muxed : 0|8@1+ (1,0) [0|255] "" Vector__XXX m0`
    let bad = || DbcError::BadSignal {
        path: origin.into(),
        line,
        text: text.into(),
    };
    let (name_part, after_name) = rest.split_once(':').ok_or_else(bad)?;
    let name = name_part.trim().to_string();
    if name.is_empty() {
        return Err(bad());
    }
    let mut tail = after_name.split_whitespace();
    // `<start>|<len>@<endian><sign>`
    let layout = tail.next().ok_or_else(bad)?;
    let (bits, endian_sign) = layout.split_once('@').ok_or_else(bad)?;
    let (start_s, len_s) = bits.split_once('|').ok_or_else(bad)?;
    let start_bit: u32 = start_s.parse().map_err(|_| bad())?;
    let len: u32 = len_s.parse().map_err(|_| bad())?;
    if !(1..=32).contains(&len) {
        return Err(DbcError::BadLength {
            path: origin.into(),
            line,
            signal: name,
            len,
        });
    }
    let mut chars = endian_sign.chars();
    let little_endian = match chars.next().ok_or_else(bad)? {
        '1' => true,
        '0' => false,
        _ => return Err(bad()),
    };
    let signed = match chars.next().ok_or_else(bad)? {
        '+' => false,
        '-' => true,
        _ => return Err(bad()),
    };
    // `(factor,offset)` — join split tokens: `(0.1,0)` arrives whole unless
    // spaced, so strip parens then split on comma.
    let scale = tail
        .next()
        .ok_or_else(bad)?
        .trim_matches(|c| c == '(' || c == ')');
    let (factor_s, offset_s) = scale.split_once(',').ok_or_else(bad)?;
    let factor: f64 = factor_s.parse().map_err(|_| bad())?;
    let offset: f64 = offset_s.parse().map_err(|_| bad())?;
    // `[min|max]`
    let range = tail
        .next()
        .ok_or_else(bad)?
        .trim_matches(|c| c == '[' || c == ']');
    let (min_s, max_s) = range.split_once('|').ok_or_else(bad)?;
    let min: f64 = min_s.parse().map_err(|_| bad())?;
    let max: f64 = max_s.parse().map_err(|_| bad())?;
    // `"unit"` then receivers; a trailing `m0`/`M` marker means multiplexed.
    let mut rest_parts: Vec<&str> = tail.collect();
    let mut multiplexed: Option<String> = None;
    if let Some(last) = rest_parts.last() {
        if *last == "M" || (last.starts_with('m') && last[1..].chars().all(|c| c.is_ascii_digit()))
        {
            multiplexed = Some(last.to_string());
            rest_parts.pop();
        }
    }
    if let Some(marker) = multiplexed {
        return Err(DbcError::Multiplexed {
            path: origin.into(),
            line,
            signal: name,
            marker,
        });
    }
    if rest_parts.is_empty() {
        return Err(bad());
    }
    let unit = rest_parts[0].trim_matches('"').to_string();
    let receivers = rest_parts[1..].iter().map(|s| s.to_string()).collect();
    Ok(DbcSignal {
        name,
        start_bit,
        len,
        little_endian,
        signed,
        factor,
        offset,
        min,
        max,
        unit,
        receivers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
VERSION "canlab-test"

NS_ :
BS_ :

BU_: ABS BMS Vector__XXX

BO_ 256 EngineData: 8 Vector__XXX
 SG_ Rpm : 0|16@1+ (0.125,0) [0|8191.875] "rpm" BMS
 SG_ CoolantTemp : 16|8@1- (1,-40) [-40|215] "degC" BMS
 SG_ Flags : 24|4@1+ (1,0) [0|15] "" ABS
 SG_ BigSpeed : 7|12@0+ (0.1,0) [0|409.5] "km/h" ABS

BO_ 512 Doors: 1 Vector__XXX
 SG_ DriverDoor : 0|1@1+ (1,0) [0|1] "" ABS
"#;

    #[test]
    fn parses_messages_signals_and_nodes() {
        let dbc = Dbc::parse(FIXTURE, "test.dbc").unwrap();
        assert_eq!(dbc.version, "canlab-test");
        assert_eq!(dbc.nodes, vec!["ABS", "BMS", "Vector__XXX"]);
        let msg = dbc.message(256).unwrap();
        assert_eq!(msg.name, "EngineData");
        assert_eq!(msg.dlc, 8);
        assert_eq!(msg.signals.len(), 4);
        let rpm = &msg.signals[0];
        assert_eq!(rpm.name, "Rpm");
        assert!((rpm.start_bit, rpm.len) == (0, 16));
        assert!(rpm.little_endian && !rpm.signed);
        assert_eq!((rpm.factor, rpm.offset), (0.125, 0.0));
        assert_eq!(rpm.unit, "rpm");
        assert_eq!(rpm.receivers, vec!["BMS"]);
        assert!(dbc.message(0x999).is_err());
    }

    #[test]
    fn decodes_little_endian_unsigned_and_signed() {
        let dbc = Dbc::parse(FIXTURE, "test.dbc").unwrap();
        // Rpm: bytes[0..2] LE = 0x1000 -> 4096 * 0.125 = 512 rpm.
        // CoolantTemp: byte[2] = 0x50 = 80 - 40 = 40 degC.
        // Flags: byte[3] low nibble 0xA = 10.
        let out = dbc
            .decode(256, &[0x00, 0x10, 0x50, 0x0A, 0, 0, 0, 0])
            .unwrap();
        assert_eq!(out[0], ("Rpm".into(), 512.0, "rpm".into()));
        assert_eq!(out[1], ("CoolantTemp".into(), 40.0, "degC".into()));
        assert_eq!(out[2].1, 10.0);
        // Signed: byte[2] = 0xFF = -1 -> -41 degC.
        let out = dbc.decode(256, &[0, 0, 0xFF, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(out[1].1, -41.0);
    }

    #[test]
    fn decodes_motorola_msb_first() {
        let dbc = Dbc::parse(FIXTURE, "test.dbc").unwrap();
        // BigSpeed: start 7, len 12 @0 = byte0, then the sawtooth continues
        // at bit 15..12 (byte1 HIGH nibble). byte0 = 0x12, byte1 = 0x30 ->
        // 0x123 = 291 -> 29.1 km/h. byte2 is untouched by this signal.
        let out = dbc.decode(256, &[0x12, 0x30, 0xFF, 0, 0, 0, 0, 0]).unwrap();
        assert_eq!(out[3].0, "BigSpeed");
        assert!((out[3].1 - 29.1).abs() < 1e-9, "got {}", out[3].1);
        // Counter-check the sawtooth's low half: byte1 = 0x40 -> 0x124.
        let out = dbc.decode(256, &[0x12, 0x40, 0, 0, 0, 0, 0, 0]).unwrap();
        assert!((out[3].1 - 29.2).abs() < 1e-9, "got {}", out[3].1);
    }

    #[test]
    fn rejects_multiplexing_bad_lines_and_short_payloads() {
        let mux = "BO_ 1 X: 8 V\n SG_ S : 0|8@1+ (1,0) [0|255] \"\" V m0\n";
        let err = Dbc::parse(mux, "m.dbc").unwrap_err();
        assert!(err.to_string().contains("multiplexing"), "{err}");
        assert!(Dbc::parse("BO_ 1 X: 8\n", "m.dbc").is_err());
        assert!(Dbc::parse("SG_ orphan : 0|1@1+ (1,0) [0|1] \"\" V\n", "m.dbc").is_err());
        assert!(Dbc::parse(
            "BO_ 1 X: 8 V\n SG_ S : 0|0@1+ (1,0) [0|1] \"\" V\n",
            "m.dbc"
        )
        .unwrap_err()
        .to_string()
        .contains("1..=32"));
        let dbc = Dbc::parse(FIXTURE, "test.dbc").unwrap();
        assert!(dbc.decode(256, &[1, 2, 3]).is_err());
        assert!(dbc.decode(0x777, &[0; 8]).is_err());
    }
}
