//! Backend interface: node specs, run outcomes, errors, log parsing.
//!
//! PROMPT.md §17 conceptually wants
//! `load/start/stop/reset/pause/resume/step/inspect/peripherals`. For the
//! headless CLI the useful subset is: describe the run ([`RenodeRunSpec`]),
//! execute it supervisively, and return ground-truth observations
//! ([`BackendOutcome`]). Interactive control (pause/step/inspect) belongs to
//! the future server/WebSocket layer and is explicitly *not* faked here.
//!
//! [`McuBackend`] is object-safe so the CLI can dispatch on it without
//! knowing which emulator is underneath.

use crate::can::frame::CanFrame;
use crate::can::id::CanId;
use std::path::PathBuf;
use thiserror::Error;

/// Execution backend selected per node (project `backend:` field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Virtual,
    Renode,
}

impl BackendKind {
    /// Parse a project `backend:` string. Unknown values are errors —
    /// never silently mapped (§72).
    pub fn parse(s: &str) -> Result<Self, BackendError> {
        match s {
            "virtual" => Ok(BackendKind::Virtual),
            "renode" => Ok(BackendKind::Renode),
            other => Err(BackendError::UnknownBackend(other.into())),
        }
    }
}

/// Actionable backend errors (PROMPT.md §68: never a bare "Backend error").
#[derive(Debug, Error)]
pub enum BackendError {
    #[error("unknown backend \"{0}\" (known: virtual, renode)")]
    UnknownBackend(String),

    #[error("Renode could not be started.\n\nPossible causes:\n- Renode is not installed\n- configured path is invalid\n- platform configuration failed\n\nLooked in: PATH and {searched}\nFix: unpack a Renode portable build as .tools/renode (see docs/getting-started.md), or set CANLAB_TOOLS=/path/to/tools.\n[View Logs: rerun with RUST_LOG=debug / check the Renode console output above]")]
    NotInstalled { searched: String },

    #[error("Renode is installed but its .NET runtime was not found.\nThe Renode launcher is a script that needs `dotnet` on PATH (Renode 1.17 targets .NET 8, roll-forward to newer majors).\nFix: install the .NET 8+ runtime repo-locally:\n  curl -sSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh\n  bash /tmp/dotnet-install.sh --channel 8.0 --runtime dotnet --install-dir ./.tools/dotnet --no-path\n  export PATH=\"$PWD/.tools/dotnet:$PATH\"\nThen rerun `canlab doctor`.")]
    DotnetMissing,

    #[error("Renode needs a .NET 8+ runtime, but only found: {found}\nFix: install the .NET 8+ runtime (see the DotnetMissing error for the repo-local one-liner) and ensure it precedes older runtimes on PATH.")]
    DotnetTooOld { found: String },

    #[error("node \"{node}\": firmware file not found: {path}\nFix: place the built firmware at a project-relative path (e.g. ./firmware/{node}.elf) — copy/paste or build it with your own toolchain, then rerun.")]
    FirmwareMissing { node: String, path: String },

    #[error("node \"{node}\": firmware is not an ELF binary: {path}\nFix: build for the target MCU (e.g. arm-none-eabi-gcc -mcpu=cortex-m3) and reference the .elf output.")]
    FirmwareNotElf { node: String, path: String },

    #[error(
        "device \"{device}\" (node \"{node}\") has no executing backend on {backend}.\n{hint}"
    )]
    UnsupportedDevice {
        device: String,
        node: String,
        backend: String,
        hint: String,
    },

    #[error("mixed backends in one run ({found}).\nThis build runs either all-virtual or all-renode nodes; cross-backend time sync is not implemented yet (explicit limitation, not silent).\nFix: set every node to the same backend.")]
    MixedBackends { found: String },

    #[error("failed to spawn Renode ({binary}): {cause}\nFix: check the path is executable and rerun `canlab doctor`.")]
    SpawnFailed { binary: String, cause: String },

    #[error("Renode exited with status {status} before producing output.\n--- Renode output (tail) ---\n{tail}\n--- end ---\nFix: check the platform description and firmware paths above.")]
    EarlyExit { status: String, tail: String },

    #[error("firmware run cancelled by the user before the budget elapsed (partial observations discarded — cancel early, import never).")]
    Cancelled,

    #[error("I/O error during backend run: {0}")]
    Io(String),
}

/// One emulated node: project node id + on-disk firmware.
#[derive(Debug, Clone)]
pub struct NodeFirmware {
    /// Project node id (e.g. `engine_ecu`).
    pub node_id: String,
    /// Renode machine name derived from the node id (sanitized).
    pub machine: String,
    /// Project device string (e.g. `stm32f103`).
    pub device: String,
    /// Absolute path to the firmware image.
    pub elf: PathBuf,
}

impl NodeFirmware {
    /// Renode machine names are quoted in generated scripts, but keep them
    /// to `[A-Za-z0-9_-]` anyway so log parsing stays unambiguous.
    pub fn machine_name(node_id: &str) -> String {
        let mut name: String = node_id
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        if name.is_empty() {
            name.push_str("node");
        }
        name
    }
}

/// A full headed-by-CanLab Renode run: one bus + its firmware nodes.
#[derive(Debug, Clone)]
pub struct RenodeRunSpec {
    pub bus_id: String,
    pub bitrate: u32,
    pub nodes: Vec<NodeFirmware>,
    /// Wall-clock budget for the emulator child process. Firmware images
    /// loop forever by nature, so the budget ending is the normal end of a
    /// run — not a failure. Raise it for slow hosts/long scenarios.
    pub run_secs: u64,
}

/// One UART line observed from a machine: `(machine, message)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedUart {
    pub machine: String,
    pub message: String,
}

/// A CAN frame observed on a machine's UART, in firmware-print order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedFrame {
    /// Project node id of the observing machine.
    pub node: String,
    pub dir: FrameDir,
    pub frame: CanFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDir {
    Tx,
    Rx,
}

/// Ground truth collected from one supervised Renode run.
#[derive(Debug, Clone)]
pub struct BackendOutcome {
    /// Every UART line captured, in capture order.
    pub uart: Vec<ObservedUart>,
    /// Parsed `CAN TX OK …` lines, in capture order.
    pub tx_frames: Vec<ObservedFrame>,
    /// Parsed `CAN RX …` lines, in capture order.
    pub rx_frames: Vec<ObservedFrame>,
    /// Full emulator stdout (kept for `--export` debugging; may be large).
    pub raw_log: String,
}

/// The backend seam. Object-safe; implemented per emulator.
pub trait McuBackend {
    /// Human name for logs/errors (e.g. `renode`).
    fn name(&self) -> &'static str;
    /// Run the spec to completion (or timeout) and return observations.
    fn run(&self, spec: &RenodeRunSpec) -> Result<BackendOutcome, BackendError>;
}

/// Parse one Renode console line into `(machine, message)`.
///
/// Renode prints UART output as
/// `[16:11:30.6444] [INFO] rx-node/usart2: [host: …|virt: …] <message>`.
/// Only UART peripherals (`usart*`/`uart*`) are accepted — `sysbus`/`cpu`
/// lines (SVD loads, vector-table guesses…) are emulator noise, not node
/// output. Returns `None` for anything else.
pub fn parse_uart_line(line: &str) -> Option<ObservedUart> {
    let (_, after_info) = line.split_once("[INFO] ")?;
    let (source, after_source) = after_info.split_once(": ")?;
    let (machine, periph) = source.split_once('/')?;
    if machine.is_empty() || machine.contains(' ') || machine.contains('[') {
        return None;
    }
    if !periph.starts_with("usart") && !periph.starts_with("uart") {
        return None;
    }
    // Message follows the trailing `[host: …|virt: …]` timestamp bracket.
    let message = match after_source.rfind("] ") {
        Some(idx) => &after_source[idx + 2..],
        None => after_source,
    };
    let message = message.trim();
    if message.is_empty() {
        return None;
    }
    Some(ObservedUart {
        machine: machine.into(),
        message: message.into(),
    })
}

/// Parse a firmware CAN report line.
///
/// TX firmware prints `CAN TX OK 0x123 [01 02 03 04 05 06 07 08]`;
/// RX firmware prints `CAN RX 0x123 [01 02 03 04 05 06 07 08]`.
/// Returns `None` for boot/init/done lines and anything unparsable —
/// callers treat unparseable as opaque UART text, never as frames.
pub fn parse_can_line(message: &str, node: &str) -> Option<ObservedFrame> {
    let (dir, rest) = if let Some(rest) = message.strip_prefix("CAN TX OK ") {
        (FrameDir::Tx, rest)
    } else {
        let rest = message.strip_prefix("CAN RX ")?;
        // `CAN RX WAIT` / `CAN RX DONE …` / `CAN RX TIMEOUT` are status, not frames.
        if rest.starts_with("0x") || rest.starts_with("0X") {
            (FrameDir::Rx, rest)
        } else {
            return None;
        }
    };
    let (id_hex, data_part) = rest.split_once(' ')?;
    let id_raw = u32::from_str_radix(
        id_hex
            .strip_prefix("0x")
            .or_else(|| id_hex.strip_prefix("0X"))?,
        16,
    )
    .ok()?;
    let data_str = data_part.trim().strip_prefix('[')?.strip_suffix(']')?;
    let mut data = Vec::new();
    if !data_str.trim().is_empty() {
        for byte in data_str.split_whitespace() {
            data.push(u8::from_str_radix(byte, 16).ok()?);
        }
    }
    if data.len() > 8 {
        return None;
    }
    let id = if id_raw <= 0x7FF {
        CanId::new_standard(id_raw as u16).ok()?
    } else if id_raw <= 0x1FFF_FFFF {
        CanId::new_extended(id_raw).ok()?
    } else {
        return None;
    };
    let frame = CanFrame::new(id, &data).ok()?;
    Some(ObservedFrame {
        node: node.into(),
        dir,
        frame,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TX_LINE: &str = "[16:11:30.6374] [INFO] tx-node/usart2: [host:   3.4s (+3.22s)|virt: 0.4s (+0.4s)] CAN TX OK 0x123 [01 02 03 04 05 06 07 08]";
    const RX_LINE: &str = "[16:11:30.6444] [INFO] rx-node/usart2: [host:     3.29s (+3.15s)|virt: 0.4s (+0.35s)] CAN RX 0x123 [01 02 03 04 05 06 07 08]";
    const BOOT_LINE: &str = "[16:11:27.4125] [INFO] tx-node/usart2: [host: 0.18s (+0.18s)|virt: 0s (+0s)] CANLAB TX BOOT";

    #[test]
    fn parses_uart_lines_and_skips_noise() {
        let u = parse_uart_line(TX_LINE).unwrap();
        assert_eq!(u.machine, "tx-node");
        assert_eq!(u.message, "CAN TX OK 0x123 [01 02 03 04 05 06 07 08]");
        let u = parse_uart_line(BOOT_LINE).unwrap();
        assert_eq!(u.message, "CANLAB TX BOOT");
        assert!(parse_uart_line("[16:11:27.3731] [INFO] tx-node: Machine started.").is_none());
        assert!(parse_uart_line("[16:11:27.3731] [INFO] tx-node/sysbus: Loading block of 852 bytes length at 0x8000000.").is_none());
        assert!(parse_uart_line(
            "[16:11:27.3731] [INFO] tx-node/cpu: Guessing VectorTableOffset value to be 0x8000000."
        )
        .is_none());
        assert!(parse_uart_line("[WARNING] sysbus: whatever").is_none());
        assert!(parse_uart_line("garbage").is_none());
    }

    #[test]
    fn parses_tx_and_rx_frames() {
        let u = parse_uart_line(TX_LINE).unwrap();
        let f = parse_can_line(&u.message, "engine_ecu").unwrap();
        assert_eq!(f.dir, FrameDir::Tx);
        assert_eq!(f.node, "engine_ecu");
        assert_eq!(format!("{}", f.frame.id), "0x123");
        assert_eq!(f.frame.dlc, 8);
        assert_eq!(f.frame.data.as_slice(), &[1, 2, 3, 4, 5, 6, 7, 8]);

        let u = parse_uart_line(RX_LINE).unwrap();
        let f = parse_can_line(&u.message, "dashboard").unwrap();
        assert_eq!(f.dir, FrameDir::Rx);
        assert_eq!(format!("{}", f.frame.id), "0x123");
    }

    #[test]
    fn rejects_status_lines_and_garbage_as_frames() {
        for msg in [
            "CANLAB TX BOOT",
            "CAN INIT OK",
            "CAN RX WAIT",
            "CAN RX DONE got=5",
            "CAN TX DONE",
            "CAN RX TIMEOUT",
            "CAN TX FAIL TSR=0x12345678",
            "CAN TX OK 0x123 [01 02 ZZ]",
            "CAN TX OK 0x1FFFFFFF9 [01]",
            "hello world",
        ] {
            assert!(parse_can_line(msg, "n").is_none(), "should reject: {msg}");
        }
    }

    #[test]
    fn machine_names_are_sanitized() {
        assert_eq!(NodeFirmware::machine_name("engine_ecu"), "engine_ecu");
        assert_eq!(NodeFirmware::machine_name("dash board!"), "dash_board_");
        assert_eq!(NodeFirmware::machine_name(""), "node");
    }

    #[test]
    fn backend_kinds_parse_and_reject() {
        assert_eq!(BackendKind::parse("virtual").unwrap(), BackendKind::Virtual);
        assert_eq!(BackendKind::parse("renode").unwrap(), BackendKind::Renode);
        assert!(BackendKind::parse("qemu").is_err());
    }
}
