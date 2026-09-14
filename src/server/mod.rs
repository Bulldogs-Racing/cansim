//! Application API layer (PROMPT.md §3).
//!
//! The GUI never touches the engine or Renode directly: it speaks the JSON
//! protocol in [`proto`] to `canlab serve`, which drives one authoritative
//! [`session::Session`]. One session per server process, shared by all
//! connections behind a `Mutex` — the browser can open the editor twice
//! without forking the simulation.

pub mod proto;
pub mod serve;
pub mod session;
