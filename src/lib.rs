//! Crate root for the CanLab simulation engine.
//!
//! Layout (Phase 0 architecture, PROMPT.md §3/§5):
//!
//! - [`can`]: protocol core — frames, identifiers, bus, arbitration, timing.
//!   Must stay independent of any MCU emulator, frontend, or hardware backend.
//! - [`simulation`]: deterministic simulation clock, event log, headless engine.
//! - [`project`]: versioned, human-readable project format (YAML/JSON) + validation.
//!
//! Backend integrations (Renode, QEMU, SocketCAN) and the visual frontend
//! build on top of these crates/modules — never the other way around.

pub mod backends;
pub mod can;
pub mod dbc;
pub mod package;
pub mod project;
pub mod server;
pub mod simulation;
