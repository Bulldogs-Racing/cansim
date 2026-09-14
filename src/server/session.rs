//! Simulation session manager (PROMPT.md §3 "Simulation Manager").
//!
//! [`Session`] owns one loaded [`Project`] and its [`Engine`]. It is the
//! single authority the future WebSocket API and GUI drive: load a project,
//! run scripted traffic, pause/stop/reset/step, inject ad-hoc frames, and
//! read the ordered, sequence-numbered event log. The GUI is a view on this
//! state — it never simulates (§78).
//!
//! Scope (explicit): virtual-backend projects only. Renode projects are
//! rejected with [`SessionError::RenodeOverApi`] — firmware runs stay in
//! `canlab simulate` until WS-supervised emulator runs land.

use crate::can::bus::{BusError, CanBusConfig};
use crate::can::frame::CanFrame;
use crate::can::id::CanId;
use crate::can::timing::SimNanos;
use crate::project::{validate_project, Project};
use crate::simulation::engine::{Engine, EngineError, EngineState};
use crate::simulation::event::SimEvent;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Actionable session errors (§68).
#[derive(Debug, Error)]
pub enum SessionError {
    #[error("no project loaded — send Load with a project file path first")]
    NoProject,
    #[error("invalid project {path}: {reasons}")]
    InvalidProject { path: String, reasons: String },
    #[error("project \"{path}\" needs the Renode backend, which the live API does not supervise yet — run it headless: canlab simulate \"{path}\"")]
    RenodeOverApi { path: String },
    #[error("unknown node \"{0}\" (loaded nodes: {1})")]
    UnknownNode(String, String),
    #[error("bad frame: {0}")]
    BadFrame(String),
    #[error("engine error: {0}")]
    Engine(String),
    #[error("I/O error: {0}")]
    Io(String),
}

impl From<EngineError> for SessionError {
    fn from(e: EngineError) -> Self {
        SessionError::Engine(e.to_string())
    }
}

impl From<BusError> for SessionError {
    fn from(e: BusError) -> Self {
        SessionError::Engine(e.to_string())
    }
}

/// Summary of one scripted run (mirrors the CLI's finished line).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub transmitted: u64,
    pub received: u64,
}

/// One loaded project: engine buses/nodes plus sender→bus routing for
/// scripted and injected frames.
#[derive(Debug)]
struct Loaded {
    path: PathBuf,
    project: Project,
    node_bus: HashMap<String, String>,
}

/// Authoritative simulation session. Not thread-safe itself — the server
/// wraps it in a `Mutex`.
#[derive(Debug)]
pub struct Session {
    loaded: Option<Loaded>,
    engine: Engine,
}

impl Session {
    pub fn new() -> Self {
        Session {
            loaded: None,
            engine: Engine::new(),
        }
    }

    /// Engine lifecycle state for status displays.
    pub fn state(&self) -> EngineState {
        self.engine.state()
    }

    pub fn project_path(&self) -> Option<&Path> {
        self.loaded.as_ref().map(|l| l.path.as_path())
    }

    pub fn project(&self) -> Option<&Project> {
        self.loaded.as_ref().map(|l| &l.project)
    }

    /// Full ordered event log; the index IS the sequence number handed to
    /// API clients (`GetEvents { sinceSeq }`).
    pub fn events(&self) -> &[SimEvent] {
        self.engine.events()
    }

    pub fn events_since(&self, since_seq: usize) -> &[SimEvent] {
        let log = self.engine.events();
        if since_seq >= log.len() {
            &[]
        } else {
            &log[since_seq..]
        }
    }

    /// Load + validate a project file and (re)build the engine: one bus per
    /// declaration, every node registered on its own bus (multi-bus works
    /// here — routing is per-node attachment, no cross-bus forwarding).
    pub fn load(&mut self, path: &Path) -> Result<(), SessionError> {
        let proj = Project::load_from_file(path).map_err(|e| SessionError::InvalidProject {
            path: path.display().to_string(),
            reasons: e.to_string(),
        })?;
        let base = path.parent().unwrap_or(Path::new("."));
        let rep = validate_project(&proj, base);
        if !rep.is_ok() {
            let reasons = rep
                .errors
                .iter()
                .map(|e| format!("[{}] {}", e.path, e.message))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(SessionError::InvalidProject {
                path: path.display().to_string(),
                reasons,
            });
        }
        if proj.nodes.iter().any(|n| n.backend == "renode") {
            return Err(SessionError::RenodeOverApi {
                path: path.display().to_string(),
            });
        }
        let mut engine = Engine::new();
        for b in &proj.buses {
            engine.add_bus(CanBusConfig::new(b.id.clone(), b.bitrate)?)?;
        }
        let mut node_bus = HashMap::new();
        for n in &proj.nodes {
            engine.register_node(&n.can.bus, &n.id)?;
            node_bus.insert(n.id.clone(), n.can.bus.clone());
        }
        self.engine = engine;
        self.loaded = Some(Loaded {
            path: path.into(),
            project: proj,
            node_bus,
        });
        Ok(())
    }

    /// Run the project's scripted traffic (or the canonical demo frame when
    /// the project declares no `messages:`), exactly like headless virtual
    /// runs. Deterministic: same project, same event log.
    pub fn start(&mut self) -> Result<RunSummary, SessionError> {
        let loaded = self.loaded.as_ref().ok_or(SessionError::NoProject)?;
        // Snapshot script + default sender/bus (borrow ends before &mut).
        let script: Vec<(String, String, CanFrame)> = if loaded.project.messages.is_empty() {
            let sender = loaded.project.nodes[0].id.clone();
            let bus = loaded.node_bus[&sender].clone();
            let frame = CanFrame::new(
                CanId::new_standard(0x123).map_err(|e| SessionError::BadFrame(e.to_string()))?,
                &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
            )
            .map_err(|e| SessionError::BadFrame(e.to_string()))?;
            vec![(bus, sender, frame)]
        } else {
            let mut script = Vec::new();
            for m in &loaded.project.messages {
                let bus = loaded
                    .node_bus
                    .get(&m.sender)
                    .ok_or_else(|| SessionError::UnknownNode(m.sender.clone(), String::new()))?;
                let id = if m.extended {
                    CanId::new_extended(m.id).map_err(|e| SessionError::BadFrame(e.to_string()))?
                } else {
                    CanId::new_standard(m.id as u16)
                        .map_err(|e| SessionError::BadFrame(e.to_string()))?
                };
                let frame = CanFrame::new(id, &m.data)
                    .map_err(|e| SessionError::BadFrame(e.to_string()))?;
                script.push((bus.clone(), m.sender.clone(), frame));
            }
            script
        };
        self.engine.start();
        let mut transmitted = 0u64;
        let mut received = 0u64;
        for (bus, sender, frame) in script {
            let out = self.engine.transmit(&bus, &sender, frame)?;
            transmitted += 1;
            received += out.receivers.len() as u64;
        }
        Ok(RunSummary {
            transmitted,
            received,
        })
    }

    /// Send one ad-hoc frame now (future "inject" button / fault tooling).
    pub fn inject(
        &mut self,
        sender: &str,
        id: u32,
        extended: bool,
        data: &[u8],
    ) -> Result<usize, SessionError> {
        let bus = {
            let loaded = self.loaded.as_ref().ok_or(SessionError::NoProject)?;
            loaded
                .node_bus
                .get(sender)
                .cloned()
                .ok_or_else(|| SessionError::UnknownNode(sender.into(), known_nodes(loaded)))?
        };
        let can_id = if extended {
            CanId::new_extended(id).map_err(|e| SessionError::BadFrame(e.to_string()))?
        } else {
            CanId::new_standard(id as u16).map_err(|e| SessionError::BadFrame(e.to_string()))?
        };
        let frame =
            CanFrame::new(can_id, data).map_err(|e| SessionError::BadFrame(e.to_string()))?;
        let out = self.engine.transmit(&bus, sender, frame)?;
        Ok(out.receivers.len())
    }

    pub fn pause(&mut self) -> Result<(), SessionError> {
        self.require_loaded()?;
        self.engine.pause();
        Ok(())
    }

    pub fn resume(&mut self) -> Result<(), SessionError> {
        self.require_loaded()?;
        self.engine.start();
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), SessionError> {
        self.require_loaded()?;
        self.engine.stop();
        Ok(())
    }

    pub fn reset(&mut self) -> Result<(), SessionError> {
        self.require_loaded()?;
        self.engine.reset();
        Ok(())
    }

    /// Advance simulated time without traffic (step button).
    pub fn step(&mut self, delta_ns: SimNanos) -> Result<SimNanos, SessionError> {
        self.require_loaded()?;
        self.engine.advance(delta_ns);
        Ok(self.engine.now())
    }

    fn require_loaded(&self) -> Result<(), SessionError> {
        if self.loaded.is_none() {
            return Err(SessionError::NoProject);
        }
        Ok(())
    }
}

fn known_nodes(loaded: &Loaded) -> String {
    loaded
        .project
        .nodes
        .iter()
        .map(|n| n.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn write_project(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
        path
    }

    const TWO_NODES: &str = r#"
version: 1
simulation: {mode: deterministic}
buses:
  - {id: vehicle_bus, type: can, bitrate: 500000, fd: false}
nodes:
  - {id: engine_ecu, device: stm32f103, backend: virtual, can: {bus: vehicle_bus}}
  - {id: dashboard, device: arduino_uno, backend: virtual, can: {bus: vehicle_bus}}
messages:
  - {sender: engine_ecu, id: 0x123, data: [1, 2, 3, 4]}
"#;

    fn tmpdir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("canlab-session-{}-{}", std::process::id(), tag));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_start_events_round_trip() {
        let dir = tmpdir("basic");
        let path = write_project(&dir, "p.canlab", TWO_NODES);
        let mut s = Session::new();
        assert!(matches!(s.start(), Err(SessionError::NoProject)));
        s.load(&path).unwrap();
        assert_eq!(s.state(), EngineState::Idle);
        let summary = s.start().unwrap();
        assert_eq!(
            summary,
            RunSummary {
                transmitted: 1,
                received: 1
            }
        );
        assert_eq!(s.state(), EngineState::Running);
        // seq 0..N indexes the engine log; since == len yields nothing.
        let all = s.events_since(0);
        assert!(!all.is_empty());
        let none = s.events_since(all.len());
        assert!(none.is_empty());
        s.pause().unwrap();
        assert_eq!(s.state(), EngineState::Paused);
        s.resume().unwrap();
        s.stop().unwrap();
        assert_eq!(s.state(), EngineState::Stopped);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_demo_frame_without_messages() {
        let dir = tmpdir("demo");
        let body = TWO_NODES
            .lines()
            .take_while(|l| !l.starts_with("messages:"))
            .collect::<Vec<_>>()
            .join("\n");
        let path = write_project(&dir, "p.canlab", &body);
        let mut s = Session::new();
        s.load(&path).unwrap();
        let summary = s.start().unwrap();
        assert_eq!(summary.transmitted, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn inject_and_step() {
        let dir = tmpdir("inject");
        let path = write_project(&dir, "p.canlab", TWO_NODES);
        let mut s = Session::new();
        s.load(&path).unwrap();
        s.start().unwrap();
        let rx = s.inject("dashboard", 0x200, false, &[9, 9]).unwrap();
        assert_eq!(rx, 1);
        assert!(s.inject("ghost", 0x100, false, &[]).is_err());
        assert!(s.inject("dashboard", 0x800, false, &[]).is_err());
        assert!(s.inject("dashboard", 0x100, false, &[0; 9]).is_err());
        let t = s.step(1000).unwrap();
        assert!(t >= 1000);
        s.reset().unwrap();
        assert_eq!(s.state(), EngineState::Idle);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_invalid_and_renode_projects() {
        let dir = tmpdir("reject");
        let bad = write_project(
            &dir,
            "bad.canlab",
            "version: 1\nsimulation: {mode: nope}\nbuses: []\nnodes: []\n",
        );
        let mut s = Session::new();
        assert!(matches!(
            s.load(&bad),
            Err(SessionError::InvalidProject { .. })
        ));
        let renode = write_project(
            &dir,
            "renode.canlab",
            TWO_NODES
                .replace("backend: virtual", "backend: renode")
                .as_str(),
        );
        assert!(matches!(
            s.load(&renode),
            Err(SessionError::RenodeOverApi { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
