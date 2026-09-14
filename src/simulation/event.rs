//! Simulation-level event log (PROMPT.md §52–§53).
//!
//! The CAN bus already emits timestamped [`BusEvent`]s; this module wraps
//! them with engine lifecycle events (`SimulationStarted/Paused/Stopped …`)
//! so headless runs, the CLI, and later the WebSocket frontend share one
//! ordered, timestamped stream instead of polling state.

use crate::can::bus::BusEvent;
use crate::can::timing::SimNanos;
use serde::{Deserialize, Serialize};

/// Engine lifecycle + bus traffic in one ordered stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimEvent {
    pub time_ns: SimNanos,
    pub kind: SimEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SimEventKind {
    SimulationStarted,
    SimulationPaused,
    SimulationStopped,
    SimulationReset,
    NodeRegistered { node: String },
    BusTraffic(BusEvent),
}

impl SimEvent {
    pub fn at(time_ns: SimNanos, kind: SimEventKind) -> Self {
        SimEvent { time_ns, kind }
    }
}
