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
use crate::project::schema::{BusDecl, NodeDecl};
use crate::project::validation::KNOWN_BACKENDS;
use crate::project::validation::KNOWN_DEVICES;
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
    #[error("project has no nodes yet — add a bus and a node before running")]
    NoNodes,
    #[error("{0} id must not be empty")]
    EmptyId(&'static str),
    #[error("bus \"{0}\" already exists")]
    DuplicateBus(String),
    #[error("unknown bus \"{0}\" (buses: {1})")]
    UnknownBus(String, String),
    #[error("cannot remove bus \"{bus}\" — still used by node(s): {nodes}")]
    BusInUse { bus: String, nodes: String },
    #[error("node \"{0}\" already exists")]
    DuplicateNode(String),
    #[error("cannot remove node \"{node}\" — still referenced by {count} scripted message(s); delete them first")]
    NodeHasMessages { node: String, count: usize },
    #[error("node id mismatch: editing \"{expected}\" but the replacement declares \"{got}\" (renames are not supported)")]
    IdMismatch { expected: String, got: String },
    #[error("unknown device \"{device}\" (known: {known})")]
    UnknownDevice { device: String, known: String },
    #[error("unknown backend \"{backend}\" (known: {known})")]
    UnknownBackend { backend: String, known: String },
    #[error("backend \"{backend}\" is not supervised by the live API; use backend \"virtual\" here, or run this project headless: canlab simulate {path}")]
    UnsupportedBackend { backend: String, path: String },
    #[error("no path to save to — send SaveProject with a \"path\" first")]
    NoSavePath,
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
/// scripted and injected frames. `path` is `None` for a never-saved blank
/// canvas (`NewProject`); `dirty` tracks unsaved edits for the GUI chip.
#[derive(Debug)]
struct Loaded {
    path: Option<PathBuf>,
    project: Project,
    node_bus: HashMap<String, String>,
    dirty: bool,
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
        self.loaded.as_ref().and_then(|l| l.path.as_deref())
    }

    /// True when the in-memory project differs from what's on disk.
    pub fn dirty(&self) -> bool {
        self.loaded.as_ref().map(|l| l.dirty).unwrap_or(false)
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
            return Err(SessionError::InvalidProject {
                path: path.display().to_string(),
                reasons: flatten(&rep),
            });
        }
        let (engine, node_bus) = build_engine(&proj, &path.display().to_string())?;
        self.engine = engine;
        self.loaded = Some(Loaded {
            path: Some(path.into()),
            project: proj,
            node_bus,
            dirty: false,
        });
        Ok(())
    }

    /// Blank unsaved canvas: version 1, default simulation, no buses/nodes.
    /// Deliberately unvalidated (an empty project is an editing state, not a
    /// runnable one) — `start` refuses with [`SessionError::NoNodes`] and
    /// `save` runs the full validator before writing.
    pub fn new_project(&mut self) {
        self.engine = Engine::new();
        self.loaded = Some(Loaded {
            path: None,
            project: Project {
                version: 1,
                // NB: SimulationDecl::default() leaves mode empty, which
                // validation rejects — a blank canvas still starts legal.
                simulation: crate::project::SimulationDecl {
                    mode: "deterministic".into(),
                },
                buses: Vec::new(),
                nodes: Vec::new(),
                messages: Vec::new(),
            },
            node_bus: HashMap::new(),
            dirty: true,
        });
    }

    pub fn add_bus(&mut self, id: &str, bitrate: u32) -> Result<(), SessionError> {
        let id = id.trim();
        if id.is_empty() {
            return Err(SessionError::EmptyId("bus"));
        }
        self.apply_edit(|ctx| {
            let proj = &mut ctx.project;
            if proj.buses.iter().any(|b| b.id == id) {
                return Err(SessionError::DuplicateBus(id.into()));
            }
            proj.buses.push(BusDecl {
                id: id.into(),
                bus_type: "can".into(),
                bitrate,
                fd: false,
            });
            Ok(())
        })
    }

    pub fn update_bus(&mut self, id: &str, bitrate: u32) -> Result<(), SessionError> {
        self.apply_edit(|ctx| {
            let proj = &mut ctx.project;
            let known = bus_list(proj);
            let bus = proj
                .buses
                .iter_mut()
                .find(|b| b.id == id)
                .ok_or_else(|| SessionError::UnknownBus(id.into(), known))?;
            bus.bitrate = bitrate;
            Ok(())
        })
    }

    pub fn remove_bus(&mut self, id: &str) -> Result<(), SessionError> {
        self.apply_edit(|ctx| {
            let proj = &mut ctx.project;
            if !proj.buses.iter().any(|b| b.id == id) {
                return Err(SessionError::UnknownBus(id.into(), bus_list(proj)));
            }
            let users: Vec<String> = proj
                .nodes
                .iter()
                .filter(|n| n.can.bus == id)
                .map(|n| n.id.clone())
                .collect();
            if !users.is_empty() {
                return Err(SessionError::BusInUse {
                    bus: id.into(),
                    nodes: users.join(", "),
                });
            }
            proj.buses.retain(|b| b.id != id);
            Ok(())
        })
    }

    pub fn add_node(&mut self, mut node: NodeDecl) -> Result<(), SessionError> {
        node.id = node.id.trim().to_string();
        self.apply_edit(|ctx| {
            check_node_decl(ctx.project, &node, None, &ctx.display)?;
            ctx.project.nodes.push(node.clone());
            Ok(())
        })
    }

    pub fn update_node(&mut self, id: &str, mut node: NodeDecl) -> Result<(), SessionError> {
        node.id = node.id.trim().to_string();
        self.apply_edit(|ctx| {
            if node.id != id {
                return Err(SessionError::IdMismatch {
                    expected: id.into(),
                    got: node.id.clone(),
                });
            }
            let pos = ctx
                .project
                .nodes
                .iter()
                .position(|n| n.id == id)
                .ok_or_else(|| {
                    SessionError::UnknownNode(id.into(), known_nodes_list(ctx.project))
                })?;
            check_node_decl(ctx.project, &node, Some(pos), &ctx.display)?;
            ctx.project.nodes[pos] = node.clone();
            Ok(())
        })
    }

    pub fn remove_node(&mut self, id: &str) -> Result<(), SessionError> {
        self.apply_edit(|ctx| {
            let proj = &mut ctx.project;
            if !proj.nodes.iter().any(|n| n.id == id) {
                return Err(SessionError::UnknownNode(id.into(), known_nodes_list(proj)));
            }
            let refs = proj.messages.iter().filter(|m| m.sender == id).count();
            if refs > 0 {
                return Err(SessionError::NodeHasMessages {
                    node: id.into(),
                    count: refs,
                });
            }
            proj.nodes.retain(|n| n.id != id);
            Ok(())
        })
    }

    /// Validate strictly, then write the project YAML. `path` overrides the
    /// loaded path (save-as); without either there is nowhere to write.
    pub fn save(&mut self, path: Option<&Path>) -> Result<(), SessionError> {
        let loaded = self.loaded.as_mut().ok_or(SessionError::NoProject)?;
        let target: PathBuf = match path {
            Some(p) => p.into(),
            None => loaded.path.clone().ok_or(SessionError::NoSavePath)?,
        };
        let base = target.parent().unwrap_or(Path::new("."));
        let rep = validate_project(&loaded.project, base);
        if !rep.is_ok() {
            return Err(SessionError::InvalidProject {
                path: target.display().to_string(),
                reasons: flatten(&rep),
            });
        }
        let yaml = loaded
            .project
            .to_yaml()
            .map_err(|e| SessionError::Io(e.to_string()))?;
        std::fs::write(&target, yaml).map_err(|e| SessionError::Io(e.to_string()))?;
        // Firmware refs resolve against the project directory, which may
        // have changed on save-as — rebuild so the engine matches the file.
        let (engine, node_bus) = build_engine(&loaded.project, &target.display().to_string())?;
        self.engine = engine;
        loaded.path = Some(target);
        loaded.node_bus = node_bus;
        loaded.dirty = false;
        Ok(())
    }

    /// Clone-mutate-rebuild-commit: an edit either lands fully (fresh engine,
    /// empty event log, dirty flag) or leaves the session untouched.
    fn apply_edit(
        &mut self,
        edit: impl FnOnce(&mut EditCtx<'_>) -> Result<(), SessionError>,
    ) -> Result<(), SessionError> {
        let loaded = self.loaded.as_ref().ok_or(SessionError::NoProject)?;
        let display = loaded
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "(unsaved)".into());
        let mut next = loaded.project.clone();
        edit(&mut EditCtx {
            project: &mut next,
            display: display.clone(),
        })?;
        let (engine, node_bus) = build_engine(&next, &display)?;
        let loaded = self.loaded.as_mut().ok_or(SessionError::NoProject)?;
        self.engine = engine;
        loaded.project = next;
        loaded.node_bus = node_bus;
        loaded.dirty = true;
        Ok(())
    }

    /// Run the project's scripted traffic (or the canonical demo frame when
    /// the project declares no `messages:`), exactly like headless virtual
    /// runs. Deterministic: same project, same event log.
    pub fn start(&mut self) -> Result<RunSummary, SessionError> {
        let loaded = self.loaded.as_ref().ok_or(SessionError::NoProject)?;
        if loaded.project.nodes.is_empty() {
            return Err(SessionError::NoNodes);
        }
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

/// Validate (file-load rules) and build a fresh engine for `proj`.
/// Intermediate editing states (no buses/nodes yet) build an empty engine;
/// strict completeness is enforced by `save`, not by every keystroke.
fn build_engine(
    proj: &Project,
    display: &str,
) -> Result<(Engine, HashMap<String, String>), SessionError> {
    if proj.nodes.iter().any(|n| n.backend == "renode") {
        return Err(SessionError::RenodeOverApi {
            path: display.into(),
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
    Ok((engine, node_bus))
}

fn flatten(rep: &crate::project::ValidationReport) -> String {
    rep.errors
        .iter()
        .map(|e| format!("[{}] {}", e.path, e.message))
        .collect::<Vec<_>>()
        .join("; ")
}

/// Scratch space for one validated edit: the candidate project plus the
/// display path used in error messages.
struct EditCtx<'a> {
    project: &'a mut Project,
    display: String,
}

/// Structural node checks shared by add/update: unique id (except the node
/// being updated), known device, live-session backend, attached bus.
fn check_node_decl(
    proj: &Project,
    node: &NodeDecl,
    except: Option<usize>,
    display: &str,
) -> Result<(), SessionError> {
    if node.id.is_empty() {
        return Err(SessionError::EmptyId("node"));
    }
    if proj
        .nodes
        .iter()
        .enumerate()
        .any(|(i, n)| n.id == node.id && Some(i) != except)
    {
        return Err(SessionError::DuplicateNode(node.id.clone()));
    }
    if !KNOWN_DEVICES.contains(&node.device.as_str()) {
        return Err(SessionError::UnknownDevice {
            device: node.device.clone(),
            known: KNOWN_DEVICES.join(", "),
        });
    }
    if node.backend == "renode" {
        return Err(SessionError::UnsupportedBackend {
            backend: node.backend.clone(),
            path: display.into(),
        });
    }
    if node.backend != "virtual" {
        return Err(SessionError::UnknownBackend {
            backend: node.backend.clone(),
            known: KNOWN_BACKENDS.join(", "),
        });
    }
    if !proj.buses.iter().any(|b| b.id == node.can.bus) {
        return Err(SessionError::UnknownBus(
            node.can.bus.clone(),
            bus_list(proj),
        ));
    }
    Ok(())
}

fn bus_list(proj: &Project) -> String {
    proj.buses
        .iter()
        .map(|b| b.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn known_nodes_list(proj: &Project) -> String {
    proj.nodes
        .iter()
        .map(|n| n.id.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn known_nodes(loaded: &Loaded) -> String {
    known_nodes_list(&loaded.project)
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

    fn node(id: &str, device: &str, bus: &str) -> NodeDecl {
        NodeDecl {
            id: id.into(),
            device: device.into(),
            backend: "virtual".into(),
            firmware: None,
            can: crate::project::CanAttachment { bus: bus.into() },
            peripherals: Vec::new(),
        }
    }

    #[test]
    fn edit_topology_rebuilds_and_tracks_dirty() {
        let mut s = Session::new();
        assert!(s.add_bus("b", 500_000).is_err()); // NoProject
        s.new_project();
        assert!(s.project_path().is_none());
        assert!(s.dirty());
        assert!(matches!(s.start(), Err(SessionError::NoNodes)));

        s.add_bus("vehicle_bus", 500_000).unwrap();
        assert!(matches!(
            s.add_bus("vehicle_bus", 500_000),
            Err(SessionError::DuplicateBus(_))
        ));
        assert!(matches!(
            s.add_bus("  ", 500_000),
            Err(SessionError::EmptyId(_))
        ));
        s.add_node(node("ecu", "stm32f103", "vehicle_bus")).unwrap();
        assert!(matches!(
            s.add_node(node("ecu", "arduino_uno", "vehicle_bus")),
            Err(SessionError::DuplicateNode(_))
        ));
        assert!(matches!(
            s.add_node(node("x", "toaster", "vehicle_bus")),
            Err(SessionError::UnknownDevice { .. })
        ));
        assert!(matches!(
            s.add_node(node("x", "stm32f103", "ghost_bus")),
            Err(SessionError::UnknownBus(_, _))
        ));
        let mut renode = node("fw", "stm32f103", "vehicle_bus");
        renode.backend = "renode".into();
        assert!(matches!(
            s.add_node(renode),
            Err(SessionError::UnsupportedBackend { .. })
        ));
        // Failed edits leave the session untouched.
        assert_eq!(s.project().unwrap().nodes.len(), 1);

        s.update_bus("vehicle_bus", 250_000).unwrap();
        assert_eq!(s.project().unwrap().buses[0].bitrate, 250_000);
        assert!(matches!(
            s.update_bus("ghost", 1),
            Err(SessionError::UnknownBus(_, _))
        ));
        assert!(matches!(
            s.update_bus("vehicle_bus", 0),
            Err(SessionError::Engine(_))
        ));

        let mut moved = node("ecu", "arduino_uno", "vehicle_bus");
        moved.firmware = Some("./firmware/ecu.bin".into());
        s.update_node("ecu", moved).unwrap();
        assert_eq!(s.project().unwrap().nodes[0].device, "arduino_uno");
        assert!(matches!(
            s.update_node("ecu", node("other", "stm32f103", "vehicle_bus")),
            Err(SessionError::IdMismatch { .. })
        ));

        // Bus in use cannot go; node without messages can.
        assert!(matches!(
            s.remove_bus("vehicle_bus"),
            Err(SessionError::BusInUse { .. })
        ));
        s.remove_node("ecu").unwrap();
        s.remove_bus("vehicle_bus").unwrap();
        assert!(s.project().unwrap().buses.is_empty());
        assert!(s.dirty());

        // Round trip through disk: save validates, clears dirty, reloads.
        let dir = tmpdir("edit");
        let target = dir.join("edited.canlab");
        assert!(matches!(
            s.save(Some(&target)),
            Err(SessionError::InvalidProject { .. })
        ));
        s.add_bus("b", 125_000).unwrap();
        s.add_node(node("n", "generic_can_node", "b")).unwrap();
        s.save(Some(&target)).unwrap();
        assert!(!s.dirty());
        let text = std::fs::read_to_string(&target).unwrap();
        let back = Project::parse(&text).unwrap();
        assert_eq!(back, *s.project().unwrap());
        let mut s2 = Session::new();
        s2.load(&target).unwrap();
        assert_eq!(s2.project().unwrap(), s.project().unwrap());
        assert!(!s2.dirty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn remove_node_with_messages_is_refused() {
        let dir = tmpdir("msgs");
        let path = write_project(&dir, "p.canlab", TWO_NODES);
        let mut s = Session::new();
        s.load(&path).unwrap();
        assert!(!s.dirty());
        match s.remove_node("engine_ecu") {
            Err(SessionError::NodeHasMessages { node, count }) => {
                assert_eq!(node, "engine_ecu");
                assert_eq!(count, 1);
            }
            other => panic!("expected NodeHasMessages, got {other:?}"),
        }
        // ...but removing an unreferenced node works and dirties the session.
        s.remove_node("dashboard").unwrap();
        assert!(s.dirty());
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
