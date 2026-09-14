//! Minimal deterministic headless engine (PROMPT.md §45, Phase 3).
//!
//! [`Engine`] owns a [`SimClock`] and one [`CanBus`] per bus id. The CLI
//! (`canlab simulate`) and future server/WebSocket layers drive it; the GUI
//! is a view on top, never the simulation itself (§78).

use crate::can::bus::{ArbitrationOutcome, BusError, CanBus, CanBusConfig, TransmissionOutcome};
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
}

impl Engine {
    pub fn new() -> Self {
        Engine {
            clock: SimClock::new(),
            buses: std::collections::BTreeMap::new(),
            log: Vec::new(),
            state: EngineState::Idle,
        }
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
            bus.clear_events();
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
