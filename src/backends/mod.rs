//! MCU backend abstraction (PROMPT.md §17, §56).
//!
//! The CAN core (`crate::can`) and the simulation engine never know which
//! backend executes a node. [`api::McuBackend`] is the seam: `virtual`
//! (deterministic logical-frame nodes, runs everywhere) and `renode`
//! (real firmware in Renode, needs the external emulator) implement it.
//! Future backends (QEMU, native, …) add a variant + an implementation —
//! never an `if renode` inside the engine.
//!
//! `renode` is treated as an external dependency (§18): managed child
//! process, generated `.resc` scripts, no forks, no hard-coded temp paths,
//! no orphaned processes (§19).

pub mod api;
pub mod renode;
