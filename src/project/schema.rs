//! Project schema: versioned YAML/JSON (`version: 1`).
//!
//! Example (PROMPT.md §20):
//!
//! ```yaml
//! version: 1
//! simulation:
//!   mode: deterministic
//! buses:
//!   - id: vehicle_bus
//!     type: can
//!     bitrate: 500000
//!     fd: false
//! nodes:
//!   - id: engine_ecu
//!     device: stm32f103
//!     backend: renode
//!     firmware: ./firmware/engine.elf
//!     can:
//!       bus: vehicle_bus
//! ```
//!
//! The optional `messages:` list is a CanLab extension for headless/CI runs:
//! scripted virtual-node traffic (Phase 3). It is backwards compatible —
//! older projects without it simply run the built-in demo frame.

use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProjectError {
    #[error("failed to parse project file: {0}")]
    Parse(String),
    #[error("unsupported project version {0} (this build supports version 1)")]
    UnsupportedVersion(u32),
    #[error("I/O error reading project file: {0}")]
    Io(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    #[serde(default)]
    pub simulation: SimulationDecl,
    #[serde(default)]
    pub buses: Vec<BusDecl>,
    #[serde(default)]
    pub nodes: Vec<NodeDecl>,
    /// Optional scripted demo traffic for headless runs.
    #[serde(default)]
    pub messages: Vec<MessageDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SimulationDecl {
    /// `deterministic` (default) or `realtime`.
    #[serde(default = "default_mode")]
    pub mode: String,
}

fn default_mode() -> String {
    "deterministic".into()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusDecl {
    pub id: String,
    /// Currently only `"can"`.
    #[serde(rename = "type")]
    pub bus_type: String,
    pub bitrate: u32,
    #[serde(default)]
    pub fd: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDecl {
    pub id: String,
    pub device: String,
    pub backend: String,
    #[serde(default)]
    pub firmware: Option<String>,
    pub can: CanAttachment,
    #[serde(default)]
    pub peripherals: Vec<PeripheralDecl>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanAttachment {
    pub bus: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeripheralDecl {
    #[serde(rename = "type")]
    pub peripheral_type: String,
    #[serde(default)]
    pub spi: Option<String>,
}

/// One scripted virtual-node frame for headless simulation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageDecl {
    pub sender: String,
    pub id: u32,
    pub data: Vec<u8>,
    /// If true, `id` is interpreted as extended 29-bit.
    #[serde(default)]
    pub extended: bool,
}

impl Project {
    pub fn load_from_file(path: &Path) -> Result<Self, ProjectError> {
        let text = std::fs::read_to_string(path).map_err(|e| ProjectError::Io(e.to_string()))?;
        Self::parse(&text)
    }

    /// Parse YAML or JSON (JSON is a subset of YAML 1.2 — one parser covers
    /// both; `.canlab` files are YAML by convention).
    pub fn parse(text: &str) -> Result<Self, ProjectError> {
        let proj: Project =
            serde_yaml::from_str(text).map_err(|e| ProjectError::Parse(e.to_string()))?;
        if proj.version != 1 {
            return Err(ProjectError::UnsupportedVersion(proj.version));
        }
        Ok(proj)
    }

    pub fn to_yaml(&self) -> Result<String, ProjectError> {
        serde_yaml::to_string(self).map_err(|e| ProjectError::Parse(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
version: 1
simulation:
  mode: deterministic
buses:
  - id: vehicle_bus
    type: can
    bitrate: 500000
    fd: false
nodes:
  - id: engine_ecu
    device: stm32f103
    backend: renode
    firmware: ./firmware/engine.elf
    can:
      bus: vehicle_bus
  - id: dashboard
    device: arduino_uno
    backend: renode
    firmware: ./firmware/dashboard.hex
    peripherals:
      - type: mcp2515
        spi: spi0
    can:
      bus: vehicle_bus
"#;

    #[test]
    fn parses_prompt_example() {
        let p = Project::parse(EXAMPLE).unwrap();
        assert_eq!(p.version, 1);
        assert_eq!(p.buses.len(), 1);
        assert_eq!(p.nodes.len(), 2);
        assert_eq!(p.nodes[1].peripherals.len(), 1);
    }

    #[test]
    fn rejects_unknown_version() {
        let bad = EXAMPLE.replace("version: 1", "version: 99");
        assert!(matches!(
            Project::parse(&bad),
            Err(ProjectError::UnsupportedVersion(99))
        ));
    }

    #[test]
    fn round_trips_yaml() {
        let p = Project::parse(EXAMPLE).unwrap();
        let yaml = p.to_yaml().unwrap();
        let q = Project::parse(&yaml).unwrap();
        assert_eq!(p, q);
    }
}
