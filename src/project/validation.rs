//! Project validation: untrusted-input checks (PROMPT.md §21).
//!
//! Rules enforced here:
//!
//! - exactly version 1 (checked at parse), non-empty unique bus/node ids
//! - bus type must be `"can"`, bitrate > 0, `fd: true` rejected with an
//!   explicit "not supported yet" error (§37 — never silently accepted)
//! - every node must attach to a declared bus
//! - `device`/`backend` must come from a known allow-list; unknown values
//!   are errors (typos fail fast instead of mis-simulating)
//! - `firmware` paths: relative only, no absolute paths, no `..` traversal,
//!   no shell metacharacters; missing files are *warnings* (virtual-node
//!   headless runs don't need real ELF/HEX yet — Phase 4 will)
//! - scripted `messages`: sender must exist, id ranges + DLC enforced

use crate::project::schema::Project;
use serde::{Deserialize, Serialize};

/// Known device types for the CAN-first MVP (§25 component library).
pub const KNOWN_DEVICES: &[&str] = &[
    "stm32f103",
    "arduino_uno",
    "teensy41",
    "mcp2515",
    "can_analyzer",
    "generic_can_node",
];

/// Known backends. Only `virtual` executes today (deterministic
/// logical-frame nodes for headless/CI); `renode` parses and validates so
/// projects can be authored now, but simulation reports it as pending
/// Phase 4 instead of pretending to emulate (§72).
pub const KNOWN_BACKENDS: &[&str] = &["virtual", "renode"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ValidationReport {
    pub errors: Vec<ValidationIssue>,
    pub warnings: Vec<ValidationIssue>,
}

impl ValidationReport {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

fn err(path: impl Into<String>, message: impl Into<String>) -> ValidationIssue {
    ValidationIssue {
        path: path.into(),
        message: message.into(),
    }
}

fn firmware_path_problem(fw: &str) -> Option<String> {
    let p = std::path::Path::new(fw);
    if p.is_absolute() {
        return Some("firmware path must be relative to the project (use ./firmware/...)".into());
    }
    if fw.contains("..") {
        return Some("firmware path must not contain \"..\" (path traversal rejected)".into());
    }
    if fw.is_empty() {
        return Some("firmware path must not be empty".into());
    }
    for ch in [';', '|', '&', '$', '`', '\n', '\r'] {
        if fw.contains(ch) {
            return Some(format!(
                "firmware path contains forbidden character {:?}",
                ch
            ));
        }
    }
    None
}

pub fn validate_project(proj: &Project) -> ValidationReport {
    let mut rep = ValidationReport::default();

    if proj.buses.is_empty() {
        rep.errors.push(err("buses", "project declares no buses"));
    }
    if proj.nodes.is_empty() {
        rep.errors.push(err("nodes", "project declares no nodes"));
    }

    let mut bus_ids = std::collections::HashSet::new();
    for (i, b) in proj.buses.iter().enumerate() {
        let at = format!("buses[{i}]");
        if b.id.trim().is_empty() {
            rep.errors.push(err(&at, "bus id must not be empty"));
        } else if !bus_ids.insert(b.id.clone()) {
            rep.errors
                .push(err(&at, format!("duplicate bus id \"{}\"", b.id)));
        }
        if b.bus_type != "can" {
            rep.errors.push(err(
                &at,
                format!("unsupported bus type \"{}\" (only \"can\")", b.bus_type),
            ));
        }
        if b.bitrate == 0 {
            rep.errors.push(err(&at, "bitrate must be positive"));
        }
        if b.fd {
            rep.errors.push(err(
                &at,
                "CAN FD is not implemented yet (§37); set fd: false",
            ));
        }
    }

    let mut node_ids = std::collections::HashSet::new();
    for (i, n) in proj.nodes.iter().enumerate() {
        let at = format!("nodes[{i}]");
        if n.id.trim().is_empty() {
            rep.errors.push(err(&at, "node id must not be empty"));
        } else if !node_ids.insert(n.id.clone()) {
            rep.errors
                .push(err(&at, format!("duplicate node id \"{}\"", n.id)));
        }
        if !KNOWN_DEVICES.contains(&n.device.as_str()) {
            rep.errors.push(err(
                &at,
                format!(
                    "unknown device \"{}\" (known: {})",
                    n.device,
                    KNOWN_DEVICES.join(", ")
                ),
            ));
        }
        if !KNOWN_BACKENDS.contains(&n.backend.as_str()) {
            rep.errors.push(err(
                &at,
                format!(
                    "unknown backend \"{}\" (known: {})",
                    n.backend,
                    KNOWN_BACKENDS.join(", ")
                ),
            ));
        }
        if !bus_ids.contains(&n.can.bus) {
            rep.errors.push(err(
                &at,
                format!("node attaches to unknown bus \"{}\"", n.can.bus),
            ));
        }
        if n.backend == "renode" {
            rep.warnings.push(err(
                &at,
                "backend \"renode\" validates but does not execute yet (Phase 4); \
                 headless runs use virtual-node behavior",
            ));
        }
        if let Some(fw) = &n.firmware {
            if let Some(problem) = firmware_path_problem(fw) {
                rep.errors.push(err(format!("{at}.firmware"), problem));
            } else if !std::path::Path::new(fw).exists() {
                rep.warnings.push(err(
                    format!("{at}.firmware"),
                    format!("firmware file \"{fw}\" not found — ok for virtual runs"),
                ));
            }
        }
        if n.device == "teensy41" {
            rep.warnings.push(err(
                &at,
                "Teensy 4.1 / FlexCAN has no executing backend yet (Phase 8); \
                 recognized but simulated as a generic CAN node",
            ));
        }
    }

    if !matches!(proj.simulation.mode.as_str(), "deterministic" | "realtime") {
        rep.errors.push(err(
            "simulation.mode",
            format!(
                "unknown mode \"{}\" (expected \"deterministic\" or \"realtime\")",
                proj.simulation.mode
            ),
        ));
    }

    for (i, m) in proj.messages.iter().enumerate() {
        let at = format!("messages[{i}]");
        if !node_ids.contains(&m.sender) {
            rep.errors
                .push(err(&at, format!("unknown sender \"{}\"", m.sender)));
        }
        let max = if m.extended { 0x1FFF_FFFF } else { 0x7FF };
        if m.id > max {
            rep.errors.push(err(
                &at,
                format!("id {:#X} out of range (max {:#X})", m.id, max),
            ));
        }
        if m.data.len() > 8 {
            rep.errors.push(err(
                &at,
                format!("payload too long ({} > 8 bytes)", m.data.len()),
            ));
        }
    }

    rep
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::schema::Project;

    const GOOD: &str = r#"
version: 1
simulation: {mode: deterministic}
buses:
  - {id: vehicle_bus, type: can, bitrate: 500000, fd: false}
nodes:
  - {id: engine_ecu, device: stm32f103, backend: virtual, firmware: ./firmware/engine.elf, can: {bus: vehicle_bus}}
  - {id: dashboard, device: arduino_uno, backend: virtual, firmware: ./firmware/dashboard.hex, can: {bus: vehicle_bus}}
"#;

    #[test]
    fn good_project_passes_with_warnings_only() {
        let p = Project::parse(GOOD).unwrap();
        let rep = validate_project(&p);
        assert!(rep.is_ok(), "unexpected errors: {:?}", rep.errors);
    }

    #[test]
    fn catches_bad_refs_and_traversal() {
        let bad = r#"
version: 1
simulation: {mode: deterministic}
buses:
  - {id: b, type: can, bitrate: 500000, fd: false}
nodes:
  - {id: n1, device: toaster, backend: virtual, firmware: ../../etc/passwd, can: {bus: nope}}
"#;
        let p = Project::parse(bad).unwrap();
        let rep = validate_project(&p);
        assert!(!rep.is_ok());
        let msgs: Vec<_> = rep.errors.iter().map(|e| e.message.clone()).collect();
        assert!(msgs.iter().any(|m| m.contains("unknown device")));
        assert!(msgs.iter().any(|m| m.contains("unknown bus")));
        assert!(msgs.iter().any(|m| m.contains("traversal")));
    }

    #[test]
    fn fd_is_rejected_explicitly() {
        let fd = GOOD.replace("fd: false", "fd: true");
        let p = Project::parse(&fd).unwrap();
        let rep = validate_project(&p);
        assert!(rep.errors.iter().any(|e| e.message.contains("CAN FD")));
    }
}
