//! Virtual CAN bus: node registry, arbitration, delivery, events.
//!
//! PROMPT.md §7 requires the bus to register/unregister nodes, accept
//! transmissions, perform arbitration, deliver winning frames, report
//! arbitration losses, manage timing, expose observer events, and support
//! fault injection and bus state. This module implements all of that at the
//! logical-frame + wire-codec level:
//!
//! - Every transmission is encoded to wire bits ([`encode_frame`]) and
//!   delivered only to controllers that are not bus-off.
//! - With no eligible acknowledger the transmitter records an ACK error
//!   (TEC += 8, subject to the passive-ACK parking exception); otherwise
//!   sender and receivers log success (TEC/REC −= 1 rules).
//! - [`CanBus::transmit_with_fault`] drives corrupted wire bits or drops
//!   the frame deterministically — the bus-level fault hook that Phase 9
//!   policies (probability, duration, affected node/frame) build on.
//! - Per-controller [`ControllerStatus`] (TEC/REC/state) is queryable for
//!   the §12 UI and the analyzer.
//!
//! Automatic retransmission after errors is intentionally *not* modelled
//! yet: the outcome reports `acked`/`error` and the retry policy belongs
//! to the controller layer (bxCAN/MCP2515/FlexCAN behaviour, later).

use super::arbitration::arbitrate_ranking;
use super::bit::Bit;
use super::bits::{decode_frame, encode_frame, wire_duration_ns};
use super::errors::{kind_of_decode_error, CanErrorKind};
use super::frame::CanFrame;
use super::state::{Confinement, ControllerStatus, ErrorState};
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
    #[error("node \"{0}\" is bus-off and cannot initiate transmission")]
    NodeBusOff(String),
    #[error("node \"{0}\" is disabled and cannot initiate transmission (re-enable it first)")]
    NodeDisabled(String),
    #[error("bus name must not be empty")]
    EmptyBusName,
    #[error("bitrate must be positive, got {0}")]
    InvalidBitrate(u32),
    #[error("no transmission requests provided")]
    EmptyArbitration,
    #[error("unknown arbitration node \"{0}\" (not registered on this bus)")]
    ArbitrationUnknownNode(String),
    #[error("fault bit offset {offset} out of range for {len}-bit wire frame")]
    FaultOffsetOutOfRange { offset: usize, len: usize },
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
    NodeRegistered {
        node: String,
    },
    NodeUnregistered {
        node: String,
    },
    NodeReset {
        node: String,
    },
    NodeDisabled {
        node: String,
    },
    NodeEnabled {
        node: String,
    },
    ArbitrationStarted {
        contenders: Vec<String>,
    },
    ArbitrationLost {
        node: String,
        winner: String,
    },
    FrameTransmitted {
        sender: String,
        frame: CanFrame,
    },
    FrameReceived {
        receiver: String,
        frame: CanFrame,
    },
    CanError {
        node: String,
        kind: CanErrorKind,
    },
    ErrorStateChanged {
        node: String,
        state: ErrorState,
    },
    FrameDropped {
        sender: String,
        frame: CanFrame,
        fault: WireFault,
    },
}

impl BusEvent {
    fn at(time_ns: SimNanos, kind: BusEventKind) -> Self {
        BusEvent { time_ns, kind }
    }
}

/// Deterministic wire fault driven by [`CanBus::transmit_with_fault`].
///
/// The bus-level hook for §35 fault injection: exact bit positions, no
/// randomness. Probabilistic policies (probability/seed/duration) layer on
/// top in Phase 9 by selecting offsets, never by changing these semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireFault {
    /// Flip the wire bit at `SOF..EOF` offset (0-based).
    FlipBit(usize),
    /// Flip a CRC-sequence bit (receivers always observe a CRC error).
    CorruptCrc,
    /// Drive nothing: the frame never reaches the bus.
    DropFrame,
}

/// Outcome of a single (uncontended) transmission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransmissionOutcome {
    pub sender: String,
    pub frame: CanFrame,
    /// All registered, non-bus-off nodes (other than the sender) that
    /// accepted the frame, sorted.
    pub receivers: Vec<String>,
    /// `true` when at least one node acknowledged (no ACK error).
    pub acked: bool,
    /// Detection kind when the transmission errored, if any.
    pub error: Option<CanErrorKind>,
    /// Policy-fired fault that produced this outcome, if any (engine
    /// fault rules; explicit single-shots report via `FaultOutcome`).
    pub fault: Option<WireFault>,
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
    pub acked: bool,
    pub error: Option<CanErrorKind>,
    pub time_ns: SimNanos,
}

/// Outcome of a faulted transmission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultOutcome {
    pub sender: String,
    pub frame: CanFrame,
    pub fault: WireFault,
    /// Detection kind observed on the wire (`None` for [`WireFault::DropFrame`]
    /// and for the ACK-slot flip, which still decodes).
    pub error: Option<CanErrorKind>,
    pub receivers: Vec<String>,
    pub time_ns: SimNanos,
}

/// Attachment state for one registered node.
#[derive(Debug)]
struct NodeAttachment {
    confinement: Confinement,
    /// Operator simulation control (§35 node faults): disabled nodes
    /// neither drive nor receive, but keep their declaration and error
    /// counters. Reset re-enables (fresh deterministic start).
    enabled: bool,
}

/// A deterministic logical-frame CAN bus with wire-codec delivery and
/// per-controller fault confinement.
#[derive(Debug)]
pub struct CanBus {
    config: CanBusConfig,
    nodes: BTreeMap<String, NodeAttachment>,
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

    /// Currently disabled nodes on this bus, sorted (for status).
    pub fn disabled_nodes(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .nodes
            .iter()
            .filter(|(_, attachment)| !attachment.enabled)
            .map(|(name, _)| name.clone())
            .collect();
        out.sort();
        out
    }

    /// TEC/REC/state triple for one controller (§12 observability).
    pub fn controller_status(&self, node: &str) -> Result<ControllerStatus, BusError> {
        self.nodes
            .get(node)
            .map(|n| n.confinement.status())
            .ok_or_else(|| BusError::UnknownNode(node.to_string()))
    }

    pub fn events(&self) -> &[BusEvent] {
        &self.events
    }

    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    /// Forget wire-time tracking (monotonicity baseline) and re-enable all
    /// nodes. Engine reset needs both: the simulation clock returns to 0,
    /// so the bus must accept t=0 again, and operator disables do not
    /// survive reset (fresh deterministic start, like reseeded fault
    /// streams). Error counters are deliberately untouched — this is
    /// simulation control, not per-CPU re-init (see `reset_node`).
    pub fn reset_time(&mut self) {
        self.events.clear();
        self.last_time_ns = 0;
        self.has_time = false;
        for attachment in self.nodes.values_mut() {
            attachment.enabled = true;
        }
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
        self.nodes.insert(
            node.clone(),
            NodeAttachment {
                confinement: Confinement::new(),
                enabled: true,
            },
        );
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

    /// CPU-driven controller re-initialisation (clears TEC/REC and bus-off
    /// recovery progress, mirroring e.g. FlexCAN re-init).
    pub fn reset_node(&mut self, node: &str, at_ns: SimNanos) -> Result<(), BusError> {
        if !self.nodes.contains_key(node) {
            return Err(BusError::UnknownNode(node.to_string()));
        }
        self.check_time(at_ns)?;
        self.nodes
            .get_mut(node)
            .expect("node exists")
            .confinement
            .reset();
        self.events.push(BusEvent::at(
            at_ns,
            BusEventKind::NodeReset {
                node: node.to_string(),
            },
        ));
        Ok(())
    }

    /// Operator enable/disable (§35 node faults): a disabled node neither
    /// drives nor receives, but stays registered with counters intact.
    /// Idempotent — re-applying the current state emits no event.
    pub fn set_node_enabled(
        &mut self,
        node: &str,
        enabled: bool,
        at_ns: SimNanos,
    ) -> Result<(), BusError> {
        let current = self
            .nodes
            .get(node)
            .map(|a| a.enabled)
            .ok_or_else(|| BusError::UnknownNode(node.to_string()))?;
        self.check_time(at_ns)?;
        if current == enabled {
            return Ok(());
        }
        let attachment = self.nodes.get_mut(node).expect("node exists");
        attachment.enabled = enabled;
        let kind = if enabled {
            BusEventKind::NodeEnabled {
                node: node.to_string(),
            }
        } else {
            BusEventKind::NodeDisabled {
                node: node.to_string(),
            }
        };
        self.events.push(BusEvent::at(at_ns, kind));
        Ok(())
    }

    /// Observe one 11-recessive-bit idle sequence on behalf of every
    /// bus-off node (128 sequences recover a node to error-active).
    pub fn note_idle_11(&mut self, at_ns: SimNanos) -> Result<(), BusError> {
        self.check_time(at_ns)?;
        let mut recovered = Vec::new();
        for (name, attachment) in self.nodes.iter_mut() {
            if attachment.confinement.note_idle_11() {
                recovered.push(name.clone());
            }
        }
        for node in recovered {
            let state = self.nodes[&node].confinement.state();
            self.events.push(BusEvent::at(
                at_ns,
                BusEventKind::ErrorStateChanged { node, state },
            ));
        }
        Ok(())
    }

    /// Transmit one frame from `sender` to every other eligible node.
    ///
    /// The frame is wire-encoded; delivery requires at least one
    /// non-bus-off acknowledger, otherwise the sender records an ACK error
    /// and nothing is delivered (no automatic retry yet — see module docs).
    pub fn transmit(
        &mut self,
        sender: &str,
        frame: CanFrame,
        at_ns: SimNanos,
    ) -> Result<TransmissionOutcome, BusError> {
        self.require_driver(sender)?;
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
        let receivers = self.eligible_receivers(sender);
        if receivers.is_empty() {
            self.record_tx_error(sender, CanErrorKind::Ack, at_ns);
            return Ok(TransmissionOutcome {
                sender: sender.to_string(),
                frame,
                receivers,
                acked: false,
                error: Some(CanErrorKind::Ack),
                fault: None,
                time_ns: at_ns,
            });
        }
        for rx in &receivers {
            self.events.push(BusEvent::at(
                at_ns,
                BusEventKind::FrameReceived {
                    receiver: rx.clone(),
                    frame: frame.clone(),
                },
            ));
        }
        self.record_tx_success(sender, &receivers, at_ns);
        Ok(TransmissionOutcome {
            sender: sender.to_string(),
            frame,
            receivers,
            acked: true,
            error: None,
            fault: None,
            time_ns: at_ns,
        })
    }

    /// Resolve one round of simultaneous transmissions: the lowest-ID frame
    /// wins (§8); losers get `ArbitrationLost` events and every other
    /// eligible node observes the winning frame.
    ///
    /// Bus-off contenders drive nothing and are ignored (they cannot
    /// initiate transmission); arbitration losses are not errors.
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

        let mut eligible: Vec<(String, CanFrame)> = Vec::with_capacity(requests.len());
        let mut first_contender: Option<String> = None;
        for (node, frame) in requests {
            if first_contender.is_none() {
                first_contender = Some(node.clone());
            }
            let attachment = self.nodes.get(node.as_str()).expect("node exists");
            if attachment.enabled && self.driver_state(&node) != ErrorState::BusOff {
                eligible.push((node, frame));
            }
        }
        if eligible.is_empty() {
            // No contender can drive: every one is bus-off or disabled.
            let first = first_contender.unwrap_or_else(|| String::from("<unknown>"));
            let disabled = self
                .nodes
                .get(first.as_str())
                .map(|a| !a.enabled)
                .unwrap_or(false);
            return Err(if disabled {
                BusError::NodeDisabled(first)
            } else {
                BusError::NodeBusOff(first)
            });
        }

        let frames: Vec<CanFrame> = eligible.iter().map(|(_, f)| f.clone()).collect();
        let (winner_idx, loser_idx) =
            arbitrate_ranking(&frames).expect("non-empty requests have a winner");

        let contenders: Vec<String> = eligible.iter().map(|(n, _)| n.clone()).collect();
        self.events.push(BusEvent::at(
            at_ns,
            BusEventKind::ArbitrationStarted { contenders },
        ));

        let (winner_node, winner_frame) = eligible[winner_idx].clone();
        let mut loser_nodes = Vec::with_capacity(loser_idx.len());
        for &i in &loser_idx {
            let loser = eligible[i].0.clone();
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
        let receivers = self.eligible_receivers(&winner_node);
        if receivers.is_empty() {
            self.record_tx_error(&winner_node, CanErrorKind::Ack, at_ns);
            return Ok(ArbitrationOutcome {
                winner_node,
                winner_frame,
                loser_nodes,
                receivers,
                acked: false,
                error: Some(CanErrorKind::Ack),
                time_ns: at_ns,
            });
        }
        for rx in &receivers {
            self.events.push(BusEvent::at(
                at_ns,
                BusEventKind::FrameReceived {
                    receiver: rx.clone(),
                    frame: winner_frame.clone(),
                },
            ));
        }
        self.record_tx_success(&winner_node, &receivers, at_ns);
        Ok(ArbitrationOutcome {
            winner_node,
            winner_frame,
            loser_nodes,
            receivers,
            acked: true,
            error: None,
            time_ns: at_ns,
        })
    }

    /// Drive a deterministically corrupted (or dropped) frame: the bus-level
    /// fault hook for §35. Receivers decode the corrupted wire; on any
    /// decode failure every receiver records the detection (REC += 1) and
    /// the sender records the transmission error (TEC += 8), and the frame
    /// is discarded with no deliveries.
    pub fn transmit_with_fault(
        &mut self,
        sender: &str,
        frame: CanFrame,
        fault: WireFault,
        at_ns: SimNanos,
    ) -> Result<FaultOutcome, BusError> {
        self.require_driver(sender)?;
        self.check_time(at_ns)?;

        if fault == WireFault::DropFrame {
            self.events.push(BusEvent::at(
                at_ns,
                BusEventKind::FrameDropped {
                    sender: sender.to_string(),
                    frame: frame.clone(),
                    fault,
                },
            ));
            return Ok(FaultOutcome {
                sender: sender.to_string(),
                frame,
                fault,
                error: None,
                receivers: Vec::new(),
                time_ns: at_ns,
            });
        }

        let enc = encode_frame(&frame);
        let mut wire = enc.wire.clone();
        match fault {
            WireFault::FlipBit(offset) => {
                if offset >= wire.len() {
                    return Err(BusError::FaultOffsetOutOfRange {
                        offset,
                        len: wire.len(),
                    });
                }
                wire[offset] = wire[offset].flipped();
            }
            WireFault::CorruptCrc => {
                // Flip the last CRC-sequence bit in the unstuffed stream and
                // re-stuff, so receivers deterministically observe a CRC
                // mismatch on otherwise well-formed framing.
                let mut tail = enc.protected.clone();
                let mut crc_bits = crate::can::crc::crc_to_bits(enc.crc);
                let last = crc_bits.len() - 1;
                crc_bits[last] = crc_bits[last].flipped();
                tail.extend(crc_bits);
                let mut stuffed = crate::can::stuffing::stuff(&tail);
                stuffed.push(crate::can::bits::TAIL_CRC_DELIM);
                stuffed.push(Bit::Recessive); // ACK slot as transmitted
                stuffed.push(Bit::Recessive); // ACK delimiter
                stuffed.extend(std::iter::repeat_n(
                    Bit::Recessive,
                    crate::can::bits::EOF_LEN,
                ));
                wire = stuffed;
            }
            WireFault::DropFrame => unreachable!("handled above"),
        }

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

        // The wire is identical for every receiver, so they all agree.
        let kind = match decode_frame(&wire) {
            Ok((_, _)) => None,
            Err(e) => Some(kind_of_decode_error(&e)),
        };
        match kind {
            None => {
                let receivers = self.eligible_receivers(sender);
                for rx in &receivers {
                    self.events.push(BusEvent::at(
                        at_ns,
                        BusEventKind::FrameReceived {
                            receiver: rx.clone(),
                            frame: frame.clone(),
                        },
                    ));
                }
                self.record_tx_success(sender, &receivers, at_ns);
                Ok(FaultOutcome {
                    sender: sender.to_string(),
                    frame,
                    fault,
                    error: None,
                    receivers,
                    time_ns: at_ns,
                })
            }
            Some(kind) => {
                self.record_tx_error(sender, kind, at_ns);
                let receivers = self.eligible_receivers(sender);
                for rx in &receivers {
                    self.record_rx_error(rx, kind, at_ns);
                }
                Ok(FaultOutcome {
                    sender: sender.to_string(),
                    frame,
                    fault,
                    error: Some(kind),
                    receivers: Vec::new(),
                    time_ns: at_ns,
                })
            }
        }
    }

    /// Exact stuffed-frame transmission time (plus inter-frame space).
    pub fn frame_duration_ns(&self, frame: &CanFrame) -> u64 {
        wire_duration_ns(frame, self.config.bitrate)
    }

    fn driver_state(&self, node: &str) -> ErrorState {
        self.nodes
            .get(node)
            .map(|n| n.confinement.state())
            .unwrap_or(ErrorState::Active)
    }

    /// Sender must be registered, enabled, and able to drive (not bus-off).
    fn require_driver(&self, sender: &str) -> Result<(), BusError> {
        let attachment = self
            .nodes
            .get(sender)
            .ok_or_else(|| BusError::UnknownNode(sender.to_string()))?;
        if !attachment.enabled {
            return Err(BusError::NodeDisabled(sender.to_string()));
        }
        if attachment.confinement.state() == ErrorState::BusOff {
            return Err(BusError::NodeBusOff(sender.to_string()));
        }
        Ok(())
    }

    /// Registered, enabled others, excluding bus-off controllers (which
    /// neither receive nor acknowledge). Sorted for determinism.
    fn eligible_receivers(&self, sender: &str) -> Vec<String> {
        let mut receivers: Vec<String> = self
            .nodes
            .iter()
            .filter(|(name, attachment)| {
                name.as_str() != sender
                    && attachment.enabled
                    && attachment.confinement.state() != ErrorState::BusOff
            })
            .map(|(name, _)| name.clone())
            .collect();
        receivers.sort();
        receivers
    }

    fn record_tx_success(&mut self, sender: &str, receivers: &[String], at_ns: SimNanos) {
        self.apply_confinement(sender, at_ns, |c| c.note_tx_success());
        for rx in receivers {
            self.apply_confinement(rx, at_ns, |c| c.note_rx_success());
        }
    }

    fn record_tx_error(&mut self, sender: &str, kind: CanErrorKind, at_ns: SimNanos) {
        self.events.push(BusEvent::at(
            at_ns,
            BusEventKind::CanError {
                node: sender.to_string(),
                kind,
            },
        ));
        self.apply_confinement(sender, at_ns, |c| c.note_tx_error(kind));
    }

    fn record_rx_error(&mut self, receiver: &str, kind: CanErrorKind, at_ns: SimNanos) {
        self.events.push(BusEvent::at(
            at_ns,
            BusEventKind::CanError {
                node: receiver.to_string(),
                kind,
            },
        ));
        self.apply_confinement(receiver, at_ns, |c| c.note_rx_error());
    }

    fn apply_confinement(
        &mut self,
        node: &str,
        at_ns: SimNanos,
        update: impl FnOnce(&mut Confinement) -> ErrorState,
    ) {
        let before = self.nodes[node].confinement.state();
        let after = update(&mut self.nodes.get_mut(node).expect("node exists").confinement);
        if after != before {
            self.events.push(BusEvent::at(
                at_ns,
                BusEventKind::ErrorStateChanged {
                    node: node.to_string(),
                    state: after,
                },
            ));
        }
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

    fn solo() -> CanBus {
        let mut bus = CanBus::new(CanBusConfig::new("solo_bus", 500_000).unwrap());
        bus.register_node("only", 0).unwrap();
        bus
    }

    fn frame_123() -> CanFrame {
        CanFrame::new(CanId::new_standard(0x123).unwrap(), &[1, 2, 3, 4]).unwrap()
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
    fn disabled_nodes_neither_drive_nor_receive() {
        let mut bus = bus2();
        // Idempotent: enabling an enabled node emits nothing.
        let before = bus.events().len();
        bus.set_node_enabled("a", true, 0).unwrap();
        assert_eq!(bus.events().len(), before);
        assert!(matches!(
            bus.set_node_enabled("ghost", false, 0),
            Err(BusError::UnknownNode(_))
        ));

        bus.set_node_enabled("b", false, 0).unwrap();
        // Disabled receivers get nothing (lone-ack error path, like bus-off).
        let out = bus.transmit("a", frame_123(), 0).unwrap();
        assert!(out.receivers.is_empty());
        // Disabled senders fail loudly.
        assert_eq!(
            bus.transmit("b", frame_123(), 0),
            Err(BusError::NodeDisabled("b".into()))
        );
        // Simultaneous rounds skip disabled contenders; all-disabled
        // rounds name the condition instead of pretending bus-off.
        let out = bus
            .transmit_simultaneous(
                vec![("a".into(), frame_123()), ("b".into(), frame_123())],
                0,
            )
            .unwrap();
        assert_eq!(out.winner_node, "a");
        assert!(out.loser_nodes.is_empty());
        bus.set_node_enabled("a", false, 0).unwrap();
        assert!(matches!(
            bus.transmit_simultaneous(vec![("a".into(), frame_123())], 0),
            Err(BusError::NodeDisabled(_))
        ));
        // Events mark both transitions; reset re-enables everything.
        let kinds: Vec<&BusEventKind> = bus.events().iter().map(|e| &e.kind).collect();
        assert!(kinds.iter().any(|k| matches!(
            k,
            BusEventKind::NodeDisabled { node } if node == "b"
        )));
        bus.reset_time();
        let out = bus.transmit("b", frame_123(), 0).unwrap();
        assert_eq!(out.receivers, vec!["a".to_string()]);
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

    #[test]
    fn successful_delivery_is_acked_with_zero_counters() {
        let mut bus = bus2();
        let out = bus.transmit("a", frame_123(), 0).unwrap();
        assert!(out.acked);
        assert_eq!(out.error, None);
        assert_eq!(out.receivers, vec!["b".to_string()]);
        assert_eq!(bus.controller_status("a").unwrap().tec, 0);
        assert_eq!(bus.controller_status("b").unwrap().rec, 0);
    }

    #[test]
    fn solo_transmission_is_an_ack_error() {
        let mut bus = solo();
        let out = bus.transmit("only", frame_123(), 0).unwrap();
        assert!(!out.acked);
        assert_eq!(out.error, Some(CanErrorKind::Ack));
        assert!(out.receivers.is_empty());
        assert_eq!(bus.controller_status("only").unwrap().tec, 8);
        assert!(bus.events().iter().any(|e| matches!(
            &e.kind,
            BusEventKind::CanError { node, kind }
            if node == "only" && *kind == CanErrorKind::Ack
        )));
    }

    #[test]
    fn lone_node_parks_at_error_passive() {
        let mut bus = solo();
        // 16 unacked frames → TEC 128 → passive; then parked by the
        // passive-ACK exception: never bus-off on missing ACKs alone.
        for i in 0..40 {
            bus.transmit("only", frame_123(), i).unwrap();
        }
        let status = bus.controller_status("only").unwrap();
        assert_eq!(status.tec, 128);
        assert_eq!(status.state, ErrorState::Passive);
    }

    #[test]
    fn corrupted_crc_is_observed_by_everyone() {
        let mut bus = bus2();
        let out = bus
            .transmit_with_fault("a", frame_123(), WireFault::CorruptCrc, 0)
            .unwrap();
        assert_eq!(out.error, Some(CanErrorKind::Crc));
        assert!(out.receivers.is_empty());
        assert_eq!(bus.controller_status("a").unwrap().tec, 8);
        assert_eq!(bus.controller_status("b").unwrap().rec, 1);
    }

    #[test]
    fn flipped_data_bit_kills_delivery_and_counts() {
        let mut bus = bus2();
        let out = bus
            .transmit_with_fault("a", frame_123(), WireFault::FlipBit(30), 0)
            .unwrap();
        assert!(out.error.is_some());
        assert!(out.receivers.is_empty());
        assert_eq!(bus.controller_status("a").unwrap().tec, 8);
        assert_eq!(bus.controller_status("b").unwrap().rec, 1);
    }

    #[test]
    fn dropped_frame_drives_nothing_and_counts_nothing() {
        let mut bus = bus2();
        let out = bus
            .transmit_with_fault("a", frame_123(), WireFault::DropFrame, 0)
            .unwrap();
        assert_eq!(out.error, None);
        assert!(out.receivers.is_empty());
        assert_eq!(bus.controller_status("a").unwrap().tec, 0);
        assert_eq!(bus.controller_status("b").unwrap().rec, 0);
        assert!(bus
            .events()
            .iter()
            .any(|e| matches!(&e.kind, BusEventKind::FrameDropped { .. })));
    }

    #[test]
    fn fault_offset_out_of_range_is_explicit() {
        let mut bus = bus2();
        assert!(matches!(
            bus.transmit_with_fault("a", frame_123(), WireFault::FlipBit(10_000), 0),
            Err(BusError::FaultOffsetOutOfRange { .. })
        ));
    }

    #[test]
    fn bus_off_node_cannot_drive_and_is_excluded() {
        let mut bus = bus2();
        for i in 0..32u64 {
            bus.transmit_with_fault("a", frame_123(), WireFault::CorruptCrc, i)
                .unwrap();
        }
        assert_eq!(
            bus.controller_status("a").unwrap().state,
            ErrorState::BusOff
        );
        // Bus-off node cannot initiate...
        assert_eq!(
            bus.transmit("a", frame_123(), 32),
            Err(BusError::NodeBusOff("a".into()))
        );
        // ...and observes nothing, not even as a receiver.
        let out = bus.transmit("b", frame_123(), 33).unwrap();
        assert!(!out.acked); // nobody left to acknowledge
        assert!(out.receivers.is_empty());
        // Recovery after 128 idle sequences.
        for t in 34..(34 + 128) {
            bus.note_idle_11(t).unwrap();
        }
        assert_eq!(
            bus.controller_status("a").unwrap().state,
            ErrorState::Active
        );
        let out = bus.transmit("a", frame_123(), 34 + 128).unwrap();
        assert!(out.acked);
    }

    #[test]
    fn state_transitions_emit_events() {
        let mut bus = solo();
        for i in 0..16u64 {
            bus.transmit("only", frame_123(), i).unwrap();
        }
        assert!(bus.events().iter().any(|e| matches!(
            &e.kind,
            BusEventKind::ErrorStateChanged { ref node, state }
            if node == "only" && *state == ErrorState::Passive
        )));
    }

    #[test]
    fn manual_reset_clears_bus_off() {
        let mut bus = solo();
        // Solo node parks at passive on ACKs; force bus-off via CRC faults
        // observed... (solo faults still count TX side only: 32 × 8 = 256).
        for i in 0..32u64 {
            bus.transmit_with_fault("only", frame_123(), WireFault::CorruptCrc, i)
                .unwrap();
        }
        assert_eq!(
            bus.controller_status("only").unwrap().state,
            ErrorState::BusOff
        );
        bus.reset_node("only", 32).unwrap();
        assert_eq!(
            bus.controller_status("only").unwrap(),
            ControllerStatus {
                tec: 0,
                rec: 0,
                state: ErrorState::Active
            }
        );
    }
}
