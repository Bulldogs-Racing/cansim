//! Minimal deterministic headless engine (PROMPT.md §45, Phase 3).
//!
//! [`Engine`] owns a [`SimClock`] and one [`CanBus`] per bus id. The CLI
//! (`canlab simulate`) and future server/WebSocket layers drive it; the GUI
//! is a view on top, never the simulation itself (§78).

use crate::can::bus::{
    ArbitrationOutcome, BusError, CanBus, CanBusConfig, FaultOutcome, TransmissionOutcome,
    WireFault,
};
use crate::can::frame::CanFrame;
use crate::can::timing::SimNanos;
use crate::simulation::clock::SimClock;
use crate::simulation::event::{SimEvent, SimEventKind};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("unknown bus \"{0}\"")]
    UnknownBus(String),
    #[error("bus \"{0}\" already exists")]
    DuplicateBus(String),
    #[error(transparent)]
    Bus(#[from] BusError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineState {
    Idle,
    Running,
    Paused,
    Stopped,
}

/// Deterministic headless simulation engine.
#[derive(Debug)]
pub struct Engine {
    clock: SimClock,
    buses: std::collections::BTreeMap<String, CanBus>,
    log: Vec<SimEvent>,
    state: EngineState,
    fault_rules: Vec<FaultRule>,
    faults_enabled: bool,
}

/// One deterministic fault policy (Phase 9 slice 2): transmissions matching
/// the optional filters draw from a per-rule seeded stream; a draw below
/// `probability` faults the frame. First matching rule wins.
#[derive(Debug, Clone)]
pub struct FaultRule {
    pub fault: WireFault,
    pub node: Option<String>,
    pub id: Option<(u32, bool)>,
    pub probability: f64,
    seed: u64,
    rng: XorShift64,
}

impl FaultRule {
    pub fn new(
        fault: WireFault,
        node: Option<String>,
        id: Option<(u32, bool)>,
        probability: f64,
        seed: u64,
    ) -> Self {
        FaultRule {
            fault,
            node,
            id,
            probability,
            seed,
            rng: XorShift64::new(seed),
        }
    }

    /// True when this rule is eligible for `(sender, frame)`; draws and
    /// reports whether the fault fires. Ineligible rules never advance
    /// their stream, so adding a non-matching rule cannot perturb others.
    fn poll(&mut self, sender: &str, frame: &CanFrame) -> bool {
        if let Some(node) = &self.node {
            if node != sender {
                return false;
            }
        }
        if let Some((id, extended)) = self.id {
            let matches = match frame.id {
                crate::can::id::CanId::Standard(n) => !extended && n as u32 == id,
                crate::can::id::CanId::Extended(n) => extended && n == id,
            };
            if !matches {
                return false;
            }
        }
        self.rng.next_f64() < self.probability
    }
}

/// SplitMix64-style deterministic stream (in-house, no dependency): good
/// statistical spread for per-frame draws, fully reproducible per seed.
#[derive(Debug, Clone)]
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        // Zero is a degenerate fixed point — mix it to a live state.
        let mixed = seed
            .wrapping_add(0x9E37_79B9_7F4A_7C15)
            .wrapping_mul(0xBF58_476D_1CE4_E5B9);
        XorShift64 {
            state: if mixed == 0 { 1 } else { mixed },
        }
    }

    fn next_u64(&mut self) -> u64 {
        // xorshift64* (Marsaglia): full-period on nonzero states.
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform draw in [0.0, 1.0): 53-bit precision, never exactly 1.0, so
    /// probability 1.0 always fires and 0.0 never does.
    fn next_f64(&mut self) -> f64 {
        const DENOM: f64 = (1u64 << 53) as f64;
        ((self.next_u64() >> 11) as f64) / DENOM
    }
}

impl Engine {
    pub fn new() -> Self {
        Engine {
            clock: SimClock::new(),
            buses: std::collections::BTreeMap::new(),
            log: Vec::new(),
            state: EngineState::Idle,
            fault_rules: Vec::new(),
            faults_enabled: true,
        }
    }

    /// Install deterministic fault policies (replaces any previous set,
    /// freshly seeded). Rules apply to every normal [`Engine::transmit`]
    /// — scripted, injected, imported, replayed — but never to explicit
    /// [`Engine::transmit_with_fault`] single-shots or arbitration rounds.
    pub fn set_fault_rules(&mut self, rules: Vec<FaultRule>) {
        self.fault_rules = rules;
    }

    /// Master switch for fault policies (default on). Replay disables it
    /// so a re-run reproduces the recorded traffic exactly instead of
    /// re-drawing faults.
    pub fn set_faults_enabled(&mut self, enabled: bool) {
        self.faults_enabled = enabled;
    }

    pub fn now(&self) -> SimNanos {
        self.clock.now()
    }

    pub fn state(&self) -> EngineState {
        self.state
    }

    pub fn events(&self) -> &[SimEvent] {
        &self.log
    }

    pub fn add_bus(&mut self, config: CanBusConfig) -> Result<(), EngineError> {
        let name = config.name.clone();
        if self.buses.contains_key(&name) {
            return Err(EngineError::DuplicateBus(name));
        }
        self.buses.insert(name, CanBus::new(config));
        Ok(())
    }

    pub fn bus_names(&self) -> Vec<String> {
        self.buses.keys().cloned().collect()
    }

    pub fn register_node(&mut self, bus: &str, node: &str) -> Result<(), EngineError> {
        let t = self.clock.now();
        let b = self
            .buses
            .get_mut(bus)
            .ok_or_else(|| EngineError::UnknownBus(bus.into()))?;
        b.register_node(node, t)?;
        self.log.push(SimEvent::at(
            t,
            SimEventKind::NodeRegistered { node: node.into() },
        ));
        Ok(())
    }

    pub fn start(&mut self) {
        self.state = EngineState::Running;
        let t = self.clock.now();
        self.log
            .push(SimEvent::at(t, SimEventKind::SimulationStarted));
    }

    pub fn pause(&mut self) {
        self.state = EngineState::Paused;
        let t = self.clock.now();
        self.log
            .push(SimEvent::at(t, SimEventKind::SimulationPaused));
    }

    pub fn stop(&mut self) {
        self.state = EngineState::Stopped;
        let t = self.clock.now();
        self.log
            .push(SimEvent::at(t, SimEventKind::SimulationStopped));
    }

    pub fn reset(&mut self) {
        self.clock.reset();
        for bus in self.buses.values_mut() {
            bus.reset_time();
        }
        // Re-seed fault streams: reset returns to the deterministic start.
        for rule in &mut self.fault_rules {
            rule.rng = XorShift64::new(rule.seed);
        }
        self.log
            .push(SimEvent::at(0, SimEventKind::SimulationReset));
        self.state = EngineState::Idle;
    }

    /// Advance the clock without traffic (pause/step support).
    pub fn advance(&mut self, delta_ns: SimNanos) {
        self.clock.advance(delta_ns);
    }

    /// Transmit one frame at the current clock time, then advance the clock
    /// by the frame's exact wire duration so scripted runs read naturally.
    pub fn transmit(
        &mut self,
        bus: &str,
        sender: &str,
        frame: CanFrame,
    ) -> Result<TransmissionOutcome, EngineError> {
        // Deterministic policies first: a firing rule converts this into a
        // faulted transmission through the same mirroring/clock path.
        if self.faults_enabled {
            let fired = self
                .fault_rules
                .iter_mut()
                .find_map(|r| r.poll(sender, &frame).then_some(r.fault));
            if let Some(fault) = fired {
                let out = self.transmit_with_fault(bus, sender, frame, fault)?;
                // Faithful summary of the fault path: clean decodes deliver
                // and acknowledge like normal transmits; errored/dropped
                // frames deliver nothing. (The lone-node clean flip inherits
                // the bus hook's behavior as-is — see transmit_with_fault.)
                return Ok(TransmissionOutcome {
                    acked: out.error.is_none() && !out.receivers.is_empty(),
                    error: out.error,
                    fault: Some(fault),
                    receivers: out.receivers.clone(),
                    sender: out.sender.clone(),
                    frame: out.frame.clone(),
                    time_ns: out.time_ns,
                });
            }
        }
        let t = self.clock.now();
        let b = self
            .buses
            .get_mut(bus)
            .ok_or_else(|| EngineError::UnknownBus(bus.into()))?;
        let before = b.events().len();
        let outcome = b.transmit(sender, frame, t)?;
        // Mirror new bus events into the engine log.
        let fresh: Vec<_> = b.events()[before..].to_vec();
        for e in fresh {
            self.log.push(SimEvent::at(t, SimEventKind::BusTraffic(e)));
        }
        self.clock.advance(b.frame_duration_ns(&outcome.frame));
        Ok(outcome)
    }

    /// Drive one deterministically faulted frame (Phase 9, single-shot):
    /// same event mirroring and clock step as [`Engine::transmit`], with the
    /// bus-level fault applied. Out-of-range offsets fail as
    /// [`BusError::FaultOffsetOutOfRange`] — never silently clamped.
    pub fn transmit_with_fault(
        &mut self,
        bus: &str,
        sender: &str,
        frame: CanFrame,
        fault: WireFault,
    ) -> Result<FaultOutcome, EngineError> {
        let t = self.clock.now();
        let b = self
            .buses
            .get_mut(bus)
            .ok_or_else(|| EngineError::UnknownBus(bus.into()))?;
        let before = b.events().len();
        let outcome = b.transmit_with_fault(sender, frame, fault, t)?;
        let fresh: Vec<_> = b.events()[before..].to_vec();
        for e in fresh {
            self.log.push(SimEvent::at(t, SimEventKind::BusTraffic(e)));
        }
        self.clock.advance(b.frame_duration_ns(&outcome.frame));
        Ok(outcome)
    }

    /// One simultaneous-transmission round at the current clock time.
    pub fn transmit_simultaneous(
        &mut self,
        bus: &str,
        requests: Vec<(String, CanFrame)>,
    ) -> Result<ArbitrationOutcome, EngineError> {
        let t = self.clock.now();
        let b = self
            .buses
            .get_mut(bus)
            .ok_or_else(|| EngineError::UnknownBus(bus.into()))?;
        let before = b.events().len();
        let outcome = b.transmit_simultaneous(requests, t)?;
        for e in b.events()[before..].iter().cloned() {
            self.log.push(SimEvent::at(t, SimEventKind::BusTraffic(e)));
        }
        self.clock
            .advance(b.frame_duration_ns(&outcome.winner_frame));
        Ok(outcome)
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::can::id::CanId;

    fn test_engine() -> Engine {
        let mut e = Engine::new();
        e.add_bus(CanBusConfig::new("b".to_string(), 500_000).unwrap())
            .unwrap();
        e.register_node("b", "a").unwrap();
        e.register_node("b", "c").unwrap();
        e.start();
        e
    }

    fn frame_123() -> CanFrame {
        CanFrame::new(CanId::new_standard(0x123).unwrap(), &[1, 2, 3, 4]).unwrap()
    }

    fn drop_all(seed: u64) -> FaultRule {
        FaultRule::new(WireFault::DropFrame, None, None, 1.0, seed)
    }

    #[test]
    fn certain_drop_delivers_nothing_but_logs_it() {
        let mut e = test_engine();
        e.set_fault_rules(vec![drop_all(7)]);
        let out = e.transmit("b", "a", frame_123()).unwrap();
        assert!(out.receivers.is_empty());
        assert!(!out.acked);
        assert!(e.events().iter().any(|ev| matches!(
            &ev.kind,
            SimEventKind::BusTraffic(crate::can::bus::BusEvent {
                kind: crate::can::bus::BusEventKind::FrameDropped { .. },
                ..
            })
        )));
    }

    #[test]
    fn zero_probability_never_fires() {
        let mut e = test_engine();
        e.set_fault_rules(vec![FaultRule::new(
            WireFault::CorruptCrc,
            None,
            None,
            0.0,
            1,
        )]);
        let out = e.transmit("b", "a", frame_123()).unwrap();
        assert_eq!(out.receivers, vec!["c".to_string()]);
        assert!(out.acked);
    }

    #[test]
    fn filters_scope_rules_and_misses_dont_consume_stream() {
        let mut e = test_engine();
        // Rule only for another node / another id: our frames pass clean,
        // and the misses must not advance the (seeded) stream.
        e.set_fault_rules(vec![
            FaultRule::new(WireFault::DropFrame, Some("ghost".into()), None, 1.0, 1),
            FaultRule::new(WireFault::DropFrame, None, Some((0x200, false)), 1.0, 2),
        ]);
        let out = e.transmit("b", "a", frame_123()).unwrap();
        assert_eq!(out.receivers, vec!["c".to_string()]);
    }

    #[test]
    fn same_seed_same_log_different_seed_likely_differs() {
        fn run_once(seed: u64) -> Vec<SimEventKind> {
            let mut e = test_engine();
            e.set_fault_rules(vec![FaultRule::new(
                WireFault::CorruptCrc,
                None,
                None,
                0.5,
                seed,
            )]);
            for _ in 0..8 {
                let _ = e.transmit("b", "a", frame_123());
            }
            e.events().iter().map(|ev| ev.kind.clone()).collect()
        }
        assert_eq!(run_once(42), run_once(42));
        // Different seeds draw different fault patterns (overwhelmingly;
        // asserted as inequality of full logs, not of one draw).
        assert_ne!(run_once(42), run_once(43));
    }

    #[test]
    fn reset_reseeds_and_disable_switches_off() {
        let mut e = test_engine();
        e.set_fault_rules(vec![FaultRule::new(
            WireFault::DropFrame,
            None,
            None,
            0.5,
            9,
        )]);
        let first = e.transmit("b", "a", frame_123()).unwrap();
        let _ = e.transmit("b", "a", frame_123());
        e.reset();
        e.start();
        let replayed = e.transmit("b", "a", frame_123()).unwrap();
        assert_eq!(first, replayed);

        e.set_faults_enabled(false);
        // Even a certain rule cannot fire while disabled.
        e.set_fault_rules(vec![drop_all(1)]);
        let out = e.transmit("b", "a", frame_123()).unwrap();
        assert_eq!(out.receivers, vec!["c".to_string()]);
    }

    #[test]
    fn rng_streams_are_sane() {
        let mut a = XorShift64::new(0);
        let mut b = XorShift64::new(0);
        for _ in 0..100 {
            let x = a.next_f64();
            assert!((0.0..1.0).contains(&x));
            assert_eq!(x, b.next_f64());
        }
        // Seed 0 is mixed to a live state (no degenerate all-zero stream).
        assert_ne!(a.next_u64(), 0);
    }
}
