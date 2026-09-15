//! Simulation-level event log (PROMPT.md §52–§53).
//!
//! The CAN bus already emits timestamped [`BusEvent`]s; this module wraps
//! them with engine lifecycle events (`SimulationStarted/Paused/Stopped …`)
//! so headless runs, the CLI, and later the WebSocket frontend share one
//! ordered, timestamped stream instead of polling state.

use crate::can::bus::{BusEvent, BusEventKind};
use crate::can::frame::CanFrame;
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

/// Replay script (§54, headless slice): every transmitted frame in log
/// order as `(sender, frame)`. RX deliveries, lifecycle markers, errors,
/// and drops are derived/observational — replaying TX alone reproduces the
/// run deterministically through a fresh engine.
pub fn extract_transmissions(events: &[SimEvent]) -> Vec<(String, CanFrame)> {
    let mut script = Vec::new();
    for e in events {
        if let SimEventKind::BusTraffic(BusEvent {
            kind: BusEventKind::FrameTransmitted { sender, frame },
            ..
        }) = &e.kind
        {
            script.push((sender.clone(), frame.clone()));
        }
    }
    script
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::can::id::CanId;

    fn tx(sender: &str, id: u16) -> SimEvent {
        let frame = CanFrame::new(CanId::new_standard(id).unwrap(), &[id as u8]).unwrap();
        SimEvent::at(
            0,
            SimEventKind::BusTraffic(BusEvent {
                time_ns: 0,
                kind: BusEventKind::FrameTransmitted {
                    sender: sender.into(),
                    frame,
                },
            }),
        )
    }

    #[test]
    fn extracts_tx_in_order_and_skips_the_rest() {
        let log = vec![
            SimEvent::at(0, SimEventKind::SimulationStarted),
            tx("a", 0x100),
            SimEvent::at(1, SimEventKind::SimulationStopped),
            tx("b", 0x200),
        ];
        let script = extract_transmissions(&log);
        assert_eq!(script.len(), 2);
        assert_eq!(script[0].0, "a");
        assert_eq!(script[1].0, "b");
        assert_eq!(format!("{}", script[1].1.id), "0x200");
        assert!(extract_transmissions(&[]).is_empty());
    }

    #[test]
    fn replay_script_survives_json_round_trip() {
        // The CLI's --export-json output is the replay input format.
        let log = vec![
            tx("ecu", 0x123),
            SimEvent::at(5, SimEventKind::SimulationStopped),
        ];
        let json = serde_json::to_string(&log).unwrap();
        let back: Vec<SimEvent> = serde_json::from_str(&json).unwrap();
        assert_eq!(extract_transmissions(&back).len(), 1);
    }
}
