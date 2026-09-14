//! Deterministic simulation support: clock, events, headless engine.

pub mod clock;
pub mod engine;
pub mod event;

pub use clock::{ClockError, SimClock};
pub use engine::{Engine, EngineError, EngineState};
pub use event::{SimEvent, SimEventKind};
