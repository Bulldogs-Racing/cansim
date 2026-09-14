//! Virtual CAN bus: node registry, arbitration, delivery, events.
//!
//! PROMPT.md §7 requires the bus to register/unregister nodes, accept
//! transmissions, perform arbitration, deliver winning frames, report
//! arbitration losses, manage timing, expose observer events, and (later)
//! support fault injection + bus state. This module implements the
//! deterministic logical-frame core; error counters / bus-off (§12),
//! CRC/stuffing (§10–§11), and fault injection (§35) build on the event
//! stream without changing delivery semantics.

use super::arbitration::arbitrate_ranking;
use super::frame::CanFrame;
use super::timing::SimNanos;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum BusError {
    #[error("node \"{0}\" is already registered on this bus")]
    DuplicateNode(String),
    #[error("unknown node \"{0}\" (not registered on this bus)")]
    UnknownNode(String),
    #[error("bus name must not be empty")]
    EmptyBusName,
    #[error("bitrate must be positive, got {0}")]
    InvalidBitrate(u32),
    #[error("no transmission requests provided")]
    EmptyArbitration,
    #[error("unknown arbitration node \"{0}\" (not registered on this bus)")]
    ArbitrationUnknownNode(String),
    #[error("timestamps must be monotonic: got {got} ns after {last} ns")]
    NonMonotonicTimestamp { got: SimNanos, last: SimNanos },
}

/// Configuration for one logical CAN bus.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanBusConfig {
    pub name: String,
    /// Nominal bitrate in bit/s, e.g. 500_000.
    pub bitrate: u32,
}

impl CanBusConfig {
    pub fn new(name: impl Into<String>, bitrate: u32) -> Result<Self, BusError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(BusError::EmptyBusName);
        }
        if bitrate == 0 {
            return Err(BusError::InvalidBitrate(bitrate));
        }
        Ok(CanBusConfig { name, bitrate })
    }
}

/// Observer-visible bus events. Every event carries a simulation timestamp
/// (§52–§53); the frontend consumes this stream instead of polling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BusEvent {
    pub time_ns: SimNanos,
    pub kind: BusEventKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum BusEventKind {
    NodeRegistered { node: String },
    NodeUnregistered { node: String },
    ArbitrationStarted { contenders: Vec<String> },
    ArbitrationLost { node: String, winner: String },
    FrameTransmitted { sender: String, frame: CanFrame },
    FrameReceived { receiver: String, frame: CanFrame },
}

impl BusEvent {
    fn at(time_ns: SimNanos, kind: BusEventKind) -> Self {
        BusEvent { time_ns, kind }
    }
}

/// Outcome of a single (uncontended) transmission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransmissionOutcome {
    pub sender: String,
    pub frame: CanFrame,
    /// All other registered nodes that observed the frame, sorted.
    pub receivers: Vec<String>,
    pub time_ns: SimNanos,
}

/// Outcome of one simultaneous-transmission (arbitration) round.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArbitrationOutcome {
    pub winner_node: String,
    pub winner_frame: CanFrame,
    /// Losers in arbitration order (closest contender first).
    pub loser_nodes: Vec<String>,
    /// Nodes that observed the winning frame (everyone except the winner).
    pub receivers: Vec<String>,
    pub time_ns: SimNanos,
}

/// A deterministic logical-frame CAN bus.
#[derive(Debug)]
pub struct CanBus {
    config: CanBusConfig,
    nodes: BTreeMap<String, ()>,
    events: Vec<BusEvent>,
    last_time_ns: SimNanos,
    has_time: bool,
}

impl CanBus {
    pub fn new(config: CanBusConfig) -> Self {
        CanBus {
            config,
            nodes: BTreeMap::new(),
            events: Vec::new(),
            last_time_ns: 0,
            has_time: false,
        }
    }

    pub fn config(&self) -> &CanBusConfig {
        &self.config
    }

    pub fn bitrate(&self) -> u32 {
        self.config.bitrate
    }

    pub fn node_names(&self) -> Vec<String> {
        self.nodes.keys().cloned().collect()
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn contains_node(&self, node: &str) -> bool {
        self.nodes.contains_key(node)
    }

    pub fn events(&self) -> &[BusEvent] {
        &self.events
    }

    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    pub fn register_node(
        &mut self,
        node: impl Into<String>,
        at_ns: SimNanos,
    ) -> Result<(), BusError> {
        let node = node.into();
        if node.trim().is_empty() {
            return Err(BusError::UnknownNode(node));
        }
        if self.nodes.contains_key(&node) {
            return Err(BusError::DuplicateNode(node));
        }
        self.check_time(at_ns)?;
        self.nodes.insert(node.clone(), ());
        self.events
            .push(BusEvent::at(at_ns, BusEventKind::NodeRegistered { node }));
        Ok(())
    }

    pub fn unregister_node(&mut self, node: &str, at_ns: SimNanos) -> Result<(), BusError> {
        if self.nodes.remove(node).is_none() {
            return Err(BusError::UnknownNode(node.to_string()));
        }
        self.check_time(at_ns)?;
        self.events.push(BusEvent::at(
            at_ns,
            BusEventKind::NodeUnregistered {
                node: node.to_string(),
            },
        ));
        Ok(())
    }

    /// Transmit one frame from `sender` to every other registered node.
    pub fn transmit(
        &mut self,
        sender: &str,
        frame: CanFrame,
        at_ns: SimNanos,
    ) -> Result<TransmissionOutcome, BusError> {
        if !self.nodes.contains_key(sender) {
            return Err(BusError::UnknownNode(sender.to_string()));
        }
        self.check_time(at_ns)?;
        self.events.push(BusEvent::at(
            at_ns,
            BusEventKind::ArbitrationStarted {
                contenders: vec![sender.to_string()],
            },
        ));
        self.events.push(BusEvent::at(
            at_ns,
            BusEventKind::FrameTransmitted {
                sender: sender.to_string(),
                frame: frame.clone(),
            },
        ));
        let mut receivers: Vec<String> = self
            .nodes
            .keys()
            .filter(|n| n.as_str() != sender)
            .cloned()
            .collect();
        receivers.sort();
        for rx in &receivers {
            self.events.push(BusEvent::at(
                at_ns,
                BusEventKind::FrameReceived {
                    receiver: rx.clone(),
                    frame: frame.clone(),
                },
            ));
        }
        Ok(TransmissionOutcome {
            sender: sender.to_string(),
            frame,
            receivers,
            time_ns: at_ns,
        })
    }

    /// Resolve one round of simultaneous transmissions: the lowest-ID frame
    /// wins (§8); losers get `ArbitrationLost` events and every other node
    /// observes the winning frame.
    pub fn transmit_simultaneous(
        &mut self,
        requests: Vec<(String, CanFrame)>,
        at_ns: SimNanos,
    ) -> Result<ArbitrationOutcome, BusError> {
        if requests.is_empty() {
            return Err(BusError::EmptyArbitration);
        }
        for (node, _) in &requests {
            if !self.nodes.contains_key(node.as_str()) {
                return Err(BusError::ArbitrationUnknownNode(node.clone()));
            }
        }
        self.check_time(at_ns)?;

        let frames: Vec<CanFrame> = requests.iter().map(|(_, f)| f.clone()).collect();
        let (winner_idx, loser_idx) =
            arbitrate_ranking(&frames).expect("non-empty requests have a winner");

        let contenders: Vec<String> = requests.iter().map(|(n, _)| n.clone()).collect();
        self.events.push(BusEvent::at(
            at_ns,
            BusEventKind::ArbitrationStarted { contenders },
        ));

        let (winner_node, winner_frame) = requests[winner_idx].clone();
        let mut loser_nodes = Vec::with_capacity(loser_idx.len());
        for &i in &loser_idx {
            let loser = requests[i].0.clone();
            self.events.push(BusEvent::at(
                at_ns,
                BusEventKind::ArbitrationLost {
                    node: loser.clone(),
                    winner: winner_node.clone(),
                },
            ));
            loser_nodes.push(loser);
        }

        self.events.push(BusEvent::at(
            at_ns,
            BusEventKind::FrameTransmitted {
                sender: winner_node.clone(),
                frame: winner_frame.clone(),
            },
        ));
        let mut receivers: Vec<String> = self
            .nodes
            .keys()
            .filter(|n| n.as_str() != winner_node.as_str())
            .cloned()
            .collect();
        receivers.sort();
        for rx in &receivers {
            self.events.push(BusEvent::at(
                at_ns,
                BusEventKind::FrameReceived {
                    receiver: rx.clone(),
                    frame: winner_frame.clone(),
                },
            ));
        }

        Ok(ArbitrationOutcome {
            winner_node,
            winner_frame,
            loser_nodes,
            receivers,
            time_ns: at_ns,
        })
    }

    fn check_time(&mut self, at_ns: SimNanos) -> Result<(), BusError> {
        if self.has_time && at_ns < self.last_time_ns {
            return Err(BusError::NonMonotonicTimestamp {
                got: at_ns,
                last: self.last_time_ns,
            });
        }
        self.last_time_ns = at_ns;
        self.has_time = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::can::id::CanId;

    fn bus2() -> CanBus {
        let mut bus = CanBus::new(CanBusConfig::new("vehicle_bus", 500_000).unwrap());
        bus.register_node("a", 0).unwrap();
        bus.register_node("b", 0).unwrap();
        bus
    }

    #[test]
    fn register_rejects_duplicates_and_unknown_tx() {
        let mut bus = bus2();
        assert_eq!(
            bus.register_node("a", 0),
            Err(BusError::DuplicateNode("a".into()))
        );
        let frame = CanFrame::new(CanId::new_standard(0x100).unwrap(), &[1]).unwrap();
        assert_eq!(
            bus.transmit("ghost", frame, 0),
            Err(BusError::UnknownNode("ghost".into()))
        );
    }

    #[test]
    fn config_validation() {
        assert_eq!(CanBusConfig::new("", 500_000), Err(BusError::EmptyBusName));
        assert_eq!(CanBusConfig::new("b", 0), Err(BusError::InvalidBitrate(0)));
    }

    #[test]
    fn timestamps_are_monotonic() {
        let mut bus = bus2();
        let frame = CanFrame::new(CanId::new_standard(0x100).unwrap(), &[1]).unwrap();
        bus.transmit("a", frame.clone(), 100).unwrap();
        assert_eq!(
            bus.transmit("a", frame, 50),
            Err(BusError::NonMonotonicTimestamp { got: 50, last: 100 })
        );
    }
}
