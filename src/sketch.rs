//! Arduino sketch import: static extraction of CAN intent (no emulation).
//!
//! Users write firmware in the Arduino IDE; CanLab never compiles or runs
//! it. Instead this module reads a sketch like a recipe — which library it
//! uses, which IDs it sends, with what DLC and payload — and turns that
//! into ordinary scripted messages for the deterministic engine. Anything
//! only knowable at runtime (sensor reads, counters) is flagged dynamic,
//! never invented.
//!
//! Supported dialects (detected from `#include` lines at runtime):
//!
//! ```text
//! <mcp_can.h>   mcp_can (MCP2515):  CAN.sendMsgBuf(id, ext, len, buf)
//! <CAN.h>       sandeepmistry arduino-CAN: CAN.beginPacket(id[, ext]) …
//!                                    CAN.write(b | buf, len) … CAN.endPacket()
//! <FlexCAN_T4.h> Teensy FlexCAN_T4 — recognized but deferred (explicit
//!                                    error, never silently parsed as above)
//! ```
//!
//! Scope (explicit): single-file line scanning plus same-file `#define` /
//! `const` / byte-array initializers. No block-comment nesting beyond
//! `/* … */` stripping, no multi-line array initializers, no `enum`
//! values, no loop-trip-count inference (a `write` inside `for` marks the
//! send dynamic). Sibling-file following (`config.h`) arrives with the
//! import flow, not here.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// Actionable sketch errors (§68): every rejection says what was found and
/// how to proceed (supported dialects, `--dialect` override).
#[derive(Debug, Error)]
pub enum SketchError {
    #[error("no supported CAN library detected (found includes: {headers}). Supported: <mcp_can.h> (mcp_can), <CAN.h> (arduino-CAN). Fix: use a supported library, or pin one with --dialect mcp_can|arduino-can")]
    UnknownLibrary { headers: String },
    #[error("unknown --dialect \"{0}\" (supported: mcp_can, arduino-can)")]
    UnknownDialect(String),
    #[error("FlexCAN_T4 (Teensy) detected via <FlexCAN_T4.h> but not implemented yet (Phase 8: no FlexCAN execution backend). Fix: use backend \"virtual\" with hand-written messages until then")]
    FlexcanDeferred,
}

/// A value that is either a resolved constant or a runtime expression the
/// parser refuses to invent (the `String` is the source text, for review).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Constness<T> {
    Const(T),
    Dynamic(String),
}

impl<T> Constness<T> {
    pub fn is_dynamic(&self) -> bool {
        matches!(self, Constness::Dynamic(_))
    }
}

/// One CAN send call-site found in a sketch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SketchSend {
    /// Display name of the dialect that produced this send.
    pub library: String,
    /// 1-based line number of the send call (packet open for arduino-CAN).
    pub line: usize,
    pub id: Constness<u32>,
    pub extended: Constness<bool>,
    pub dlc: Constness<u8>,
    pub data: Vec<Constness<u8>>,
}

impl SketchSend {
    /// True when every value resolved to a constant (directly simulatable).
    pub fn is_concrete(&self) -> bool {
        matches!(self.id, Constness::Const(_))
            && matches!(self.extended, Constness::Const(_))
            && matches!(self.dlc, Constness::Const(_))
            && self.data.iter().all(|b| !b.is_dynamic())
    }
}

/// What the importer found: detected dialects plus sends in file order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SketchIntent {
    pub detected: Vec<String>,
    pub sends: Vec<SketchSend>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dialect {
    McpCan,
    ArduinoCan,
}

impl Dialect {
    fn display(self) -> &'static str {
        match self {
            Dialect::McpCan => "mcp_can",
            Dialect::ArduinoCan => "arduino-can",
        }
    }

    fn from_override(name: &str) -> Result<Dialect, SketchError> {
        match name.to_ascii_lowercase().as_str() {
            "mcp_can" | "mcp-can" => Ok(Dialect::McpCan),
            "arduino-can" | "arduino_can" | "arduino can" => Ok(Dialect::ArduinoCan),
            other => Err(SketchError::UnknownDialect(other.into())),
        }
    }
}

/// Strip `/* … */` block comments (newlines preserved, so offsets stay
/// line-stable) for pre-processing. Non-nested, like C itself.
fn strip_block_comments(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < bytes.len() {
                if bytes[i] == b'\n' {
                    out.push('\n');
                    i += 1;
                } else if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                    i += 2;
                    break;
                } else {
                    out.push(' ');
                    i += 1;
                }
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Cut a `//` line comment, ignoring `//` inside double-quoted strings.
fn strip_line_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut prev_backslash = false;
    for (i, c) in line.char_indices() {
        if in_string {
            if c == '"' && !prev_backslash {
                in_string = false;
            }
            prev_backslash = c == '\\' && !prev_backslash;
        } else if c == '"' {
            in_string = true;
            prev_backslash = false;
        } else if c == '/' && line[i + 1..].starts_with('/') {
            return &line[..i];
        }
    }
    line
}

/// Byte offset → 1-based line number.
fn line_starts(source: &str) -> Vec<usize> {
    let mut starts = vec![0];
    for (i, c) in source.char_indices() {
        if c == '\n' {
            starts.push(i + 1);
        }
    }
    starts
}

/// Mask of real-code bytes: false inside `"…"` strings and `//` comments
/// (block comments are stripped beforehand). Used to reject phantom
/// matches like `Serial.println("calling sendMsgBuf")`.
fn code_mask(source: &str) -> Vec<bool> {
    let mut mask = vec![true; source.len()];
    let bytes = source.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'"' {
            mask[i] = false;
            i += 1;
            while i < bytes.len() {
                mask[i] = false;
                if bytes[i] == b'\\' {
                    i += 1;
                    if i < bytes.len() {
                        mask[i] = false;
                        i += 1;
                    }
                } else if bytes[i] == b'"' {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
        } else if bytes[i] == b'/' && bytes.get(i + 1) == Some(&b'/') {
            while i < bytes.len() && bytes[i] != b'\n' {
                mask[i] = false;
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    mask
}

/// True when `name` at `abs` is real code (not string/comment) with a
/// non-identifier char before it (`.`/`>`/`:` method syntax allowed —
/// only alphanumerics and `_` would mean a longer identifier).
fn is_call_at(source: &str, mask: &[bool], abs: usize, name: &str) -> bool {
    if !mask[abs..abs + name.len()].iter().all(|&b| b) {
        return false;
    }
    if abs > 0 {
        let c = source[..abs].chars().next_back().unwrap();
        if c.is_alphanumeric() || c == '_' {
            return false;
        }
    }
    source[abs + name.len()..].trim_start().starts_with('(')
}

fn line_of(starts: &[usize], offset: usize) -> usize {
    match starts.binary_search(&offset) {
        Ok(n) => n + 1,
        Err(n) => n,
    }
}

/// Split a call's argument list on top-level commas (nested parens like
/// `sizeof(buf)` stay intact).
fn split_top_level_commas(args: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut current = String::new();
    for c in args.chars() {
        match c {
            '(' => {
                depth += 1;
                current.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(c);
            }
            ',' if depth == 0 => {
                parts.push(current.trim().to_string());
                current = String::new();
            }
            _ => current.push(c),
        }
    }
    parts.push(current.trim().to_string());
    parts
}

/// Find `name(` call spans as (args_start, args_end) byte offsets into
/// comment-stripped source. Only real-code, word-boundary matches count.
fn call_arg_spans(source: &str, mask: &[bool], name: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut search_from = 0;
    while let Some(found) = source[search_from..].find(name) {
        let abs = search_from + found;
        if !is_call_at(source, mask, abs, name) {
            search_from = abs + name.len();
            continue;
        }
        let after = abs + name.len();
        let paren_at = after + (source[after..].len() - source[after..].trim_start().len());
        let mut depth = 0usize;
        let mut end = None;
        for (i, c) in source[paren_at..].char_indices() {
            if c == '(' {
                depth += 1;
            } else if c == ')' {
                depth -= 1;
                if depth == 0 {
                    end = Some(paren_at + i);
                    break;
                }
            }
        }
        if let Some(close) = end {
            spans.push((paren_at + 1, close));
            search_from = close + 1;
        } else {
            search_from = abs + name.len();
        }
    }
    spans
}

/// Find the first real-code occurrence of `name` at or after `from`.
/// Returns its absolute offset.
fn find_call(source: &str, mask: &[bool], from: usize, name: &str) -> Option<usize> {
    let mut search_from = from;
    while let Some(found) = source[search_from..].find(name) {
        let abs = search_from + found;
        // `endPacket` takes no arguments: only the boundary matters here.
        let before_ok = if abs == 0 {
            true
        } else {
            let c = source[..abs].chars().next_back().unwrap();
            !(c.is_alphanumeric() || c == '_')
        };
        if before_ok && mask[abs..abs + name.len()].iter().all(|&b| b) {
            return Some(abs);
        }
        search_from = abs + name.len();
    }
    None
}

/// Parse an integer literal with Arduino-flavored affixes: hex `0x`,
/// binary `0b`, octal `0`, decimal, trailing `u`/`l` suffixes, parens.
fn parse_int_literal(text: &str) -> Option<u64> {
    let mut t = text.trim();
    while t.starts_with('(') && t.ends_with(')') && t.len() > 2 {
        t = t[1..t.len() - 1].trim();
    }
    let t = t.trim_end_matches(['u', 'U', 'l', 'L']);
    if t.is_empty() {
        return None;
    }
    let (radix, digits) = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        (16, h)
    } else if let Some(b) = t.strip_prefix("0b").or_else(|| t.strip_prefix("0B")) {
        (2, b)
    } else if t.len() > 1 && t.starts_with('0') && t.chars().all(|c| c.is_ascii_digit()) {
        (8, &t[1..])
    } else {
        (10, t)
    };
    if digits.is_empty() || digits.starts_with(['+', '-']) {
        return None;
    }
    u64::from_str_radix(digits, radix).ok()
}

/// Same-file constant table: `#define NAME value` (last wins) plus
/// one-line `const`/`constexpr TYPE NAME = value;`.
fn collect_defines(source: &str) -> HashMap<String, String> {
    let mut defines = HashMap::new();
    for raw_line in source.lines() {
        let line = strip_line_comment(raw_line).trim();
        if let Some(rest) = line.strip_prefix('#') {
            let rest = rest.trim_start();
            if let Some(body) = rest.strip_prefix("define") {
                // `#define` needs following whitespace (else `#defineX`).
                let Some(after) = body.strip_prefix([' ', '\t']) else {
                    continue;
                };
                let mut parts = after.splitn(2, [' ', '\t']);
                let name = parts.next().unwrap_or("").trim();
                // Function-like macros (`#define F(x) …`) are out of scope.
                if name.is_empty() || name.contains('(') {
                    continue;
                }
                if let Some(value) = parts.next() {
                    let value = value.trim();
                    if !value.is_empty() {
                        defines.insert(name.to_string(), value.to_string());
                    }
                }
            }
            continue;
        }
        // `const uint8_t DLC = 8;` / `constexpr int ID = 0x100;`
        let line = line.trim_end_matches(';').trim();
        let body = line
            .strip_prefix("const ")
            .or_else(|| line.strip_prefix("constexpr "));
        if let Some(body) = body {
            if let Some((decl, value)) = body.split_once('=') {
                let mut words = decl.split_whitespace();
                if let Some(name) = words.next_back() {
                    if name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        defines.insert(name.to_string(), value.trim().to_string());
                    }
                }
            }
        }
    }
    defines
}

/// Resolve an expression to a constant via defines (recursive, one level —
/// `#define A B` chains resolve because values re-resolve on lookup).
fn resolve_int(expr: &str, defines: &HashMap<String, String>) -> Option<u64> {
    let expr = expr.trim();
    if let Some(n) = parse_int_literal(expr) {
        return Some(n);
    }
    if expr.chars().all(|c| c.is_alphanumeric() || c == '_') {
        if let Some(value) = defines.get(expr) {
            if value != expr {
                return resolve_int(value, defines);
            }
        }
    }
    None
}

fn constness_u32(expr: &str, defines: &HashMap<String, String>) -> Constness<u32> {
    match resolve_int(expr, defines) {
        Some(n) if n <= u32::MAX as u64 => Constness::Const(n as u32),
        _ => Constness::Dynamic(expr.trim().to_string()),
    }
}

/// Same-file byte-array initializers: `byte NAME[N] = {…};` (single line).
/// Each element resolves independently, so `engineData[0] = counter++`
/// later can demote single bytes to dynamic (see `apply_index_writes`).
fn collect_arrays(
    source: &str,
    defines: &HashMap<String, String>,
) -> HashMap<String, Vec<Constness<u8>>> {
    let mut arrays = HashMap::new();
    for raw_line in source.lines() {
        let line = strip_line_comment(raw_line).trim();
        let Some((decl, init)) = line.split_once('=') else {
            continue;
        };
        let init = init.trim().trim_end_matches(';').trim();
        let Some(inner) = init.strip_prefix('{').and_then(|s| s.strip_suffix('}')) else {
            continue;
        };
        let mut words: Vec<&str> = decl.split_whitespace().collect();
        let Some(last) = words.pop() else { continue };
        // Last word looks like `NAME`, `NAME[8]`, or `NAME[]`.
        let name = last.split('[').next().unwrap_or("").trim();
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        // First significant word(s) must be a byte type (`static`/`const`
        // qualifiers filtered out; `unsigned char` spans two words).
        let sig: Vec<&str> = words
            .into_iter()
            .filter(|w| *w != "static" && *w != "const")
            .collect();
        let is_byte = matches!(
            sig.as_slice(),
            ["byte"]
                | ["uint8_t"]
                | ["int8_t"]
                | ["char"]
                | ["unsigned", "char"]
                | ["signed", "char"]
        );
        if !is_byte {
            continue;
        }
        let mut bytes = Vec::new();
        for element in split_top_level_commas(inner) {
            if element.trim().is_empty() {
                continue;
            }
            match resolve_int(&element, defines) {
                Some(n) if n <= u8::MAX as u64 => bytes.push(Constness::Const(n as u8)),
                _ => bytes.push(Constness::Dynamic(element.trim().to_string())),
            }
        }
        arrays.insert(name.to_string(), bytes);
    }
    arrays
}

/// Demote bytes overwritten by `NAME[i] = expr;` writes to dynamic.
/// Non-constant indices demote the whole array (conservative, explicit).
fn apply_index_writes(source: &str, arrays: &mut HashMap<String, Vec<Constness<u8>>>) {
    for raw_line in source.lines() {
        let line = strip_line_comment(raw_line).trim();
        let Some(bracket) = line.find('[') else {
            continue;
        };
        // Skip declarations (`byte a[8] = …`) and comparisons (`==`).
        let head = line[..bracket].trim();
        if head.contains(' ') || head.is_empty() {
            continue;
        }
        let Some(rest) = line[bracket..].split_once(']') else {
            continue;
        };
        let after = rest.1.trim_start();
        let Some(rhs) = after.strip_prefix('=').filter(|s| !s.starts_with('=')) else {
            continue;
        };
        let Some(bytes) = arrays.get_mut(head) else {
            continue;
        };
        let index_expr = rest.0[1..].trim();
        match parse_int_literal(index_expr) {
            Some(i) if (i as usize) < bytes.len() => {
                bytes[i as usize] =
                    Constness::Dynamic(rhs.trim().trim_end_matches(';').trim().to_string());
            }
            _ => {
                for b in bytes.iter_mut() {
                    *b = Constness::Dynamic(format!("{head}[{index_expr}]"));
                }
            }
        }
    }
}

/// `sizeof(NAME)` resolves against known arrays; anything else is dynamic.
fn resolve_sizeof(expr: &str, arrays: &HashMap<String, Vec<Constness<u8>>>) -> Option<usize> {
    let expr = expr.trim();
    let inner = expr.strip_prefix("sizeof")?.trim_start();
    let inner = inner.strip_prefix('(')?.strip_suffix(')')?.trim();
    arrays.get(inner).map(|b| b.len())
}

/// Scan `#include` lines → (known headers, flexcan seen).
fn scan_includes(source: &str) -> (Vec<String>, bool) {
    let mut headers = Vec::new();
    let mut flexcan = false;
    for raw_line in source.lines() {
        let line = strip_line_comment(raw_line).trim();
        let Some(rest) = line.strip_prefix('#') else {
            continue;
        };
        let rest = rest.trim_start();
        let Some(body) = rest.strip_prefix("include") else {
            continue;
        };
        // `#include` must be followed by whitespace (not `#include_next`).
        if !body.starts_with([' ', '\t']) {
            continue;
        }
        let body = body.trim_start();
        let header = body
            .strip_prefix('<')
            .and_then(|s| s.split_once('>').map(|(h, _)| h))
            .or_else(|| {
                body.strip_prefix('"')
                    .and_then(|s| s.split_once('"').map(|(h, _)| h))
            });
        if let Some(header) = header {
            let file = header
                .rsplit('/')
                .next()
                .unwrap_or(header)
                .to_ascii_lowercase();
            if file == "flexcan_t4.h" {
                flexcan = true;
            } else {
                headers.push(file);
            }
        }
    }
    (headers, flexcan)
}

/// Map known headers to dialects (call-site presence confirms, see
/// [`extract_can_intent`]).
fn dialects_for_headers(headers: &[String]) -> Vec<Dialect> {
    let mut dialects = Vec::new();
    for header in headers {
        let dialect = match header.as_str() {
            "mcp_can.h" => Some(Dialect::McpCan),
            "can.h" => Some(Dialect::ArduinoCan),
            _ => None,
        };
        if let Some(d) = dialect {
            if !dialects.contains(&d) {
                dialects.push(d);
            }
        }
    }
    dialects
}

fn constness_bool(expr: &str, defines: &HashMap<String, String>) -> Constness<bool> {
    let t = expr.trim();
    if t == "true" {
        return Constness::Const(true);
    }
    if t == "false" {
        return Constness::Const(false);
    }
    match resolve_int(t, defines) {
        Some(n) => Constness::Const(n != 0),
        None => Constness::Dynamic(t.to_string()),
    }
}

fn extract_mcp_can(
    stripped: &str,
    mask: &[bool],
    starts: &[usize],
    defines: &HashMap<String, String>,
    arrays: &HashMap<String, Vec<Constness<u8>>>,
) -> Vec<SketchSend> {
    let mut sends = Vec::new();
    for (args_start, args_end) in call_arg_spans(stripped, mask, "sendMsgBuf") {
        let args = split_top_level_commas(&stripped[args_start..args_end]);
        if args.len() != 4 {
            continue;
        }
        let data = match arrays.get(args[3].trim()) {
            Some(bytes) => bytes.clone(),
            None => {
                // Unknown buffer (pointer, return value): length from a
                // constant DLC, contents dynamic — never invented.
                let len = match resolve_int(&args[2], defines) {
                    Some(n) if n <= 8 => n as usize,
                    _ => 0,
                };
                vec![Constness::Dynamic(format!("{}[i]", args[3].trim())); len]
            }
        };
        sends.push(SketchSend {
            library: Dialect::McpCan.display().into(),
            line: line_of(starts, args_start),
            id: constness_u32(&args[0], defines),
            extended: constness_bool(&args[1], defines),
            dlc: match resolve_int(&args[2], defines) {
                Some(n) if n <= u8::MAX as u64 => Constness::Const(n as u8),
                _ => Constness::Dynamic(args[2].trim().to_string()),
            },
            data,
        });
    }
    sends
}

fn extract_arduino_can(
    stripped: &str,
    mask: &[bool],
    starts: &[usize],
    defines: &HashMap<String, String>,
    arrays: &HashMap<String, Vec<Constness<u8>>>,
) -> Vec<SketchSend> {
    let mut sends = Vec::new();
    for (open_start, open_end) in call_arg_spans(stripped, mask, "beginPacket") {
        let open_args = split_top_level_commas(&stripped[open_start..open_end]);
        if open_args.is_empty() || open_args.len() > 2 {
            continue;
        }
        // Packet body runs to the matching endPacket; control flow inside
        // (for/while/if) makes the byte count unknowable — mark dynamic.
        let Some(end_abs) = find_call(stripped, mask, open_end, "endPacket") else {
            continue;
        };
        let body = &stripped[open_end..end_abs];
        let body_mask = &mask[open_end..end_abs];
        let has_control_flow = ["for", "while", "if", "switch"].iter().any(|kw| {
            body.split(|c: char| !(c.is_alphanumeric() || c == '_'))
                .any(|w| w == *kw)
        });
        let mut data = Vec::new();
        let mut dynamic_body = has_control_flow;
        for (w_start, w_end) in call_arg_spans(body, body_mask, "write") {
            let w_args = split_top_level_commas(&body[w_start..w_end]);
            match w_args.as_slice() {
                [single] => match resolve_int(single, defines) {
                    Some(n) if n <= u8::MAX as u64 => data.push(Constness::Const(n as u8)),
                    _ => {
                        data.push(Constness::Dynamic(single.trim().to_string()));
                        dynamic_body = true;
                    }
                },
                [buf, len] => {
                    let buf = buf.trim();
                    let chunk = match arrays.get(buf) {
                        Some(bytes) => {
                            let take = match resolve_sizeof(len, arrays) {
                                Some(n) => n.min(bytes.len()),
                                None => match resolve_int(len, defines) {
                                    Some(n) => (n as usize).min(bytes.len()),
                                    None => {
                                        dynamic_body = true;
                                        bytes.len()
                                    }
                                },
                            };
                            bytes[..take].to_vec()
                        }
                        None => {
                            dynamic_body = true;
                            match resolve_sizeof(len, arrays)
                                .or_else(|| resolve_int(len, defines).map(|n| n as usize))
                            {
                                Some(n) if n <= 8 => {
                                    vec![Constness::Dynamic(format!("{buf}[i]")); n]
                                }
                                _ => Vec::new(),
                            }
                        }
                    };
                    data.extend(chunk);
                }
                _ => {
                    dynamic_body = true;
                }
            }
        }
        let dlc = if dynamic_body {
            Constness::Dynamic("packet body".into())
        } else {
            Constness::Const(data.len().min(255) as u8)
        };
        sends.push(SketchSend {
            library: Dialect::ArduinoCan.display().into(),
            line: line_of(starts, open_start),
            id: constness_u32(&open_args[0], defines),
            extended: open_args
                .get(1)
                .map(|e| constness_bool(e, defines))
                .unwrap_or(Constness::Const(false)),
            dlc,
            data,
        });
    }
    sends
}

/// Extract CAN intent from sketch source.
///
/// `dialect_override` pins one dialect (`mcp_can` / `arduino-can`);
/// otherwise dialects come from `#include` lines, confirmed by actual
/// send call-sites (a header without sends yields nothing, not an error).
pub fn extract_can_intent(
    source: &str,
    dialect_override: Option<&str>,
) -> Result<SketchIntent, SketchError> {
    let dialects = match dialect_override {
        Some(name) => vec![Dialect::from_override(name)?],
        None => {
            let stripped_for_scan = strip_block_comments(source);
            let (headers, flexcan) = scan_includes(&stripped_for_scan);
            let dialects = dialects_for_headers(&headers);
            if dialects.is_empty() {
                if flexcan {
                    return Err(SketchError::FlexcanDeferred);
                }
                let mut shown = headers;
                shown.sort();
                shown.dedup();
                return Err(SketchError::UnknownLibrary {
                    headers: if shown.is_empty() {
                        "(none)".into()
                    } else {
                        shown.join(", ")
                    },
                });
            }
            dialects
        }
    };
    let stripped = strip_block_comments(source);
    let mask = code_mask(&stripped);
    let starts = line_starts(&stripped);
    let defines = collect_defines(&stripped);
    let mut arrays = collect_arrays(&stripped, &defines);
    apply_index_writes(&stripped, &mut arrays);
    let mut sends = Vec::new();
    for dialect in &dialects {
        match dialect {
            Dialect::McpCan => sends.extend(extract_mcp_can(
                &stripped, &mask, &starts, &defines, &arrays,
            )),
            Dialect::ArduinoCan => sends.extend(extract_arduino_can(
                &stripped, &mask, &starts, &defines, &arrays,
            )),
        }
    }
    sends.sort_by_key(|s| s.line);
    Ok(SketchIntent {
        detected: dialects.iter().map(|d| d.display().to_string()).collect(),
        sends,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MCP_CAN_SKETCH: &str = r#"
#include <SPI.h>
#include <mcp_can.h>

#define CAN_ID 0x100
#define CAN_CS_PIN 10

MCP_CAN CAN(CAN_CS_PIN);

byte engineData[8] = {0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08};
byte counter = 0;

void setup() {
  while (CAN.begin(MCP_ANY, CAN_500KBPS, MCP_16MHZ) != CAN_OK) {
  }
  CAN.setMode(MCP_NORMAL);
}

void loop() {
  engineData[0] = counter++;
  CAN.sendMsgBuf(CAN_ID, 0, 8, engineData);
  delay(100);
}
"#;

    const ARDUINO_CAN_SKETCH: &str = r#"
#include <CAN.h>

#define DASH_ID 0x200

byte dashData[4] = {0x20, 0x01, 0x00, 0x00};

void setup() {
  if (!CAN.begin(500E3)) {
    while (1) {
    }
  }
}

void loop() {
  CAN.beginPacket(DASH_ID);
  CAN.write(dashData, sizeof(dashData));
  CAN.endPacket();
  delay(200);
}
"#;

    #[test]
    fn detects_dialect_from_includes() {
        assert_eq!(
            extract_can_intent(MCP_CAN_SKETCH, None).unwrap().detected,
            vec!["mcp_can"]
        );
        assert_eq!(
            extract_can_intent(ARDUINO_CAN_SKETCH, None)
                .unwrap()
                .detected,
            vec!["arduino-can"]
        );
        // Commented-out includes do not count.
        let commented = "// #include <mcp_can.h>\nvoid setup() {}\n";
        assert!(matches!(
            extract_can_intent(commented, None).unwrap_err(),
            SketchError::UnknownLibrary { .. }
        ));
        // Unknown headers error naming the fix.
        let err = extract_can_intent("#include <other.h>\n", None).unwrap_err();
        assert!(err.to_string().contains("--dialect"), "{err}");
        // FlexCAN is recognized but deferred, never misparsed.
        let err = extract_can_intent("#include <FlexCAN_T4.h>\n", None).unwrap_err();
        assert!(matches!(err, SketchError::FlexcanDeferred));
        // Override pins a dialect regardless of includes.
        let intent = extract_can_intent("void loop() {}\n", Some("mcp_can")).unwrap();
        assert_eq!(intent.detected, vec!["mcp_can"]);
        assert!(intent.sends.is_empty());
        assert!(matches!(
            extract_can_intent("", Some("toaster")).unwrap_err(),
            SketchError::UnknownDialect(_)
        ));
    }

    #[test]
    fn extracts_mcp_can_send_with_dynamic_byte() {
        let intent = extract_can_intent(MCP_CAN_SKETCH, None).unwrap();
        assert_eq!(intent.sends.len(), 1);
        let send = &intent.sends[0];
        assert_eq!(send.library, "mcp_can");
        assert_eq!(send.id, Constness::Const(0x100));
        assert_eq!(send.extended, Constness::Const(false));
        assert_eq!(send.dlc, Constness::Const(8));
        assert_eq!(send.data.len(), 8);
        // Byte 0 was overwritten by `counter++`: dynamic, with the expr.
        assert_eq!(send.data[0], Constness::Dynamic("counter++".into()));
        assert_eq!(send.data[1], Constness::Const(0x02));
        assert!(!send.is_concrete());
        assert!(send.line > 0);
    }

    #[test]
    fn extracts_arduino_can_packet_with_sizeof() {
        let intent = extract_can_intent(ARDUINO_CAN_SKETCH, None).unwrap();
        assert_eq!(intent.sends.len(), 1);
        let send = &intent.sends[0];
        assert_eq!(send.library, "arduino-can");
        assert_eq!(send.id, Constness::Const(0x200));
        assert_eq!(send.extended, Constness::Const(false));
        assert_eq!(send.dlc, Constness::Const(4));
        assert_eq!(
            send.data,
            vec![
                Constness::Const(0x20),
                Constness::Const(0x01),
                Constness::Const(0x00),
                Constness::Const(0x00),
            ]
        );
        assert!(send.is_concrete());
    }

    #[test]
    fn int_literals_cover_arduino_spellings() {
        assert_eq!(parse_int_literal("0x100"), Some(0x100));
        assert_eq!(parse_int_literal("0b101"), Some(5));
        assert_eq!(parse_int_literal("010"), Some(8));
        assert_eq!(parse_int_literal("42u"), Some(42));
        assert_eq!(parse_int_literal(" 8 "), Some(8));
        assert_eq!(parse_int_literal("(0x10)"), Some(0x10));
        assert_eq!(parse_int_literal("rpm / 100"), None);
        assert_eq!(parse_int_literal(""), None);
    }

    #[test]
    fn loops_mark_packets_dynamic_not_silent() {
        let sketch = "#include <CAN.h>\nvoid loop() {\n  CAN.beginPacket(0x300);\n  for (int i = 0; i < 4; i++) CAN.write(i);\n  CAN.endPacket();\n}\n";
        let intent = extract_can_intent(sketch, None).unwrap();
        assert_eq!(intent.sends.len(), 1);
        assert!(!intent.sends[0].is_concrete());
        assert!(matches!(intent.sends[0].dlc, Constness::Dynamic(_)));
    }

    /// The checked-in example sketches are the fixtures: parsing the files
    /// must agree with the inline vectors above (catches fixture drift).
    #[test]
    fn example_files_parse_as_documented() {
        let mcp = std::fs::read_to_string("firmware/examples/arduino/mcp_can_sender.ino")
            .expect("example sketch must exist");
        let intent = extract_can_intent(&mcp, None).unwrap();
        assert_eq!(intent.detected, vec!["mcp_can"]);
        assert_eq!(intent.sends.len(), 1);
        assert_eq!(intent.sends[0].id, Constness::Const(0x100));
        assert_eq!(intent.sends[0].data.len(), 8);
        assert_eq!(
            intent.sends[0].data[0],
            Constness::Dynamic("counter++".into())
        );

        let ac = std::fs::read_to_string("firmware/examples/arduino/arduino_can_sender.ino")
            .expect("example sketch must exist");
        let intent = extract_can_intent(&ac, None).unwrap();
        assert_eq!(intent.detected, vec!["arduino-can"]);
        assert_eq!(intent.sends.len(), 1);
        assert!(intent.sends[0].is_concrete());
    }
}
