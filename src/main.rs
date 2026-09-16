//! `canlab` CLI (PROMPT.md §46).
//!
//! - `canlab new <name>` — scaffold a portable project directory (§48)
//! - `canlab simulate <project>` — headless deterministic run, no GUI (§45)
//! - `canlab replay <project> <trace.json>` — deterministically replay a
//!   recorded event log (from `simulate --export-json`), no GUI (§54)
//! - `canlab validate <project>` — schema + safety checks (§21)
//! - `canlab dbc <file.dbc> --id 0x100 --data ...` — decode one payload (§38)
//! - `canlab serve [--port N] [--project p]` — local WebSocket API for the
//!   visual editor (§4); the GUI drives this, never the engine directly
//! - `canlab doctor` — environment diagnostics (Renode, toolchains, SocketCAN)

use cansimcan::can::bus::{CanBusConfig, WireFault};
use cansimcan::can::frame::CanFrame;
use cansimcan::can::id::CanId;
use cansimcan::can::timing::NS_PER_MS;
use cansimcan::project::{validate_project, Project};
use cansimcan::simulation::engine::Engine;
use cansimcan::simulation::event::{extract_replay_script, SimEvent};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "canlab",
    version,
    about = "CanLab — visual CAN network & MCU simulator (headless CLI)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scaffold a new portable project directory.
    New {
        /// Project directory to create.
        name: PathBuf,
    },
    /// Package a project directory for sharing (project file + firmware).
    Package {
        /// Project file to package.
        project: PathBuf,
        /// Output directory (must not exist).
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Run a headless deterministic simulation (no GUI required).
    Simulate {
        /// Project file (YAML, e.g. project.canlab).
        project: PathBuf,
        /// Write the timestamped event log as JSON to this path.
        #[arg(long)]
        export_json: Option<PathBuf>,
        /// Wall-clock budget in seconds for Renode firmware runs.
        /// Firmware loops forever by nature, so the budget ending is the
        /// normal end of a run — observations are reported, not an error.
        #[arg(long, default_value_t = 30)]
        run_secs: u64,
    },
    /// Validate a project file without simulating.
    Validate { project: PathBuf },
    /// Extract CAN sends from an Arduino sketch (static analysis, never
    /// compiled or run). Prints detected dialects plus a sends table, and
    /// with --as a messages: YAML snippet for project files.
    Sketch {
        /// Sketch file (.ino/.cpp).
        file: PathBuf,
        /// Pin the dialect (mcp_can | arduino-can) instead of detecting it
        /// from #include lines.
        #[arg(long)]
        dialect: Option<String>,
        /// Attribute emitted YAML rows to this node (enables the snippet).
        #[arg(long = "as")]
        as_node: Option<String>,
    },
    /// Decode one CAN payload with a DBC file (Phase 12 slice 1).
    Dbc {
        /// DBC file (message/signal layout).
        dbc: PathBuf,
        /// Message id in hex (e.g. 0x100).
        #[arg(long)]
        id: String,
        /// Payload bytes in hex (e.g. "00 10 50 0A 00 00 00 00").
        #[arg(long)]
        data: String,
    },
    /// Replay a recorded event-log trace deterministically (no GUI).
    Replay {
        /// Project file (routing: buses + nodes for the recorded senders).
        project: PathBuf,
        /// Event-log JSON from `canlab simulate --export-json`, or a
        /// SocketCAN pcap (same link type the GUI exports).
        trace: PathBuf,
        /// Write the replayed event log as JSON to this path.
        #[arg(long)]
        export_json: Option<PathBuf>,
        /// Attribute every pcap packet to this node (required for pcap
        /// traces, which carry no sender; ignored for JSON traces).
        #[arg(long = "as")]
        as_node: Option<String>,
    },
    /// Serve the local WebSocket API for the visual editor.
    Serve {
        /// Loopback port to listen on.
        #[arg(long, default_value_t = 21011)]
        port: u16,
        /// Project file to preload into the session.
        #[arg(long)]
        project: Option<PathBuf>,
        /// Max concurrent background Renode firmware runs (job table).
        #[arg(long, default_value_t = 1)]
        max_jobs: u16,
    },
    /// Diagnose the environment: Renode, toolchains, SocketCAN, …
    Doctor,
    /// Open a project in the visual editor (not implemented in this build).
    Open { project: PathBuf },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.command {
        Commands::New { name } => cmd_new(&name),
        Commands::Package { project, out } => cmd_package(&project, out.as_deref()),
        Commands::Simulate {
            project,
            export_json,
            run_secs,
        } => cmd_simulate(&project, export_json.as_deref(), run_secs),
        Commands::Validate { project } => cmd_validate(&project),
        Commands::Sketch {
            file,
            dialect,
            as_node,
        } => cmd_sketch(&file, dialect.as_deref(), as_node.as_deref()),
        Commands::Dbc { dbc, id, data } => cmd_dbc(&dbc, &id, &data),
        Commands::Replay {
            project,
            trace,
            export_json,
            as_node,
        } => cmd_replay(&project, &trace, export_json.as_deref(), as_node.as_deref()),
        Commands::Serve {
            port,
            project,
            max_jobs,
        } => cansimcan::server::serve::serve(port, project.as_deref(), max_jobs),
        Commands::Doctor => cmd_doctor(),
        Commands::Open { project } => {
            eprintln!(
                "the desktop `open` shortcut is not implemented yet — start the visual editor manually:\n\
                 \n\
                 Terminal 1: cargo run -q --bin canlab -- serve --project {}\n\
                 Terminal 2: cd frontend && npm install && npm run dev\n\
                 \n\
                 Then open the printed Vite URL and press Connect.",
                project.display(),
            );
            1
        }
    };
    std::process::exit(code);
}

const TEMPLATE: &str = r#"version: 1

simulation:
  mode: deterministic

buses:
  - id: vehicle_bus
    type: can
    bitrate: 500000
    fd: false

nodes:
  - id: engine_ecu
    device: stm32f103
    backend: virtual
    firmware: ./firmware/engine.elf
    can:
      bus: vehicle_bus

  - id: dashboard
    device: arduino_uno
    backend: virtual
    firmware: ./firmware/dashboard.hex
    peripherals:
      - type: mcp2515
        spi: spi0
    can:
      bus: vehicle_bus
"#;

fn cmd_new(dir: &Path) -> i32 {
    if dir.exists() {
        eprintln!("cannot create project: {} already exists", dir.display());
        return 1;
    }
    for sub in ["firmware", "dbc", "assets"] {
        if let Err(e) = std::fs::create_dir_all(dir.join(sub)) {
            eprintln!("failed to create {}: {e}", dir.join(sub).display());
            return 1;
        }
    }
    if let Err(e) = std::fs::write(dir.join("project.canlab"), TEMPLATE) {
        eprintln!("failed to write project file: {e}");
        return 1;
    }
    println!("Created {} with project.canlab", dir.display());
    println!(
        "Next: edit project.canlab, then run `canlab simulate {}/project.canlab`",
        dir.display()
    );
    0
}

fn load_and_validate(path: &Path) -> Option<Project> {
    let proj = match Project::load_from_file(path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("failed to load {}: {e}", path.display());
            return None;
        }
    };
    let base_dir = path.parent().unwrap_or(Path::new("."));
    let rep = validate_project(&proj, base_dir);
    for w in &rep.warnings {
        eprintln!("warning [{}]: {}", w.path, w.message);
    }
    if !rep.is_ok() {
        eprintln!("project {} is invalid:", path.display());
        for e in &rep.errors {
            eprintln!("  error [{}]: {}", e.path, e.message);
        }
        return None;
    }
    Some(proj)
}

/// Package a project for sharing: validate, then copy the project file +
/// every referenced firmware file (+ `dbc/`/`assets/` when present) into
/// a fresh output directory, preserving relative layout.
fn cmd_package(project: &Path, out: Option<&Path>) -> i32 {
    use cansimcan::package::package_project;
    let out_dir: PathBuf = match out {
        Some(o) => o.into(),
        None => {
            let stem = project
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "project".into());
            project
                .parent()
                .unwrap_or(Path::new("."))
                .join(format!("{stem}-pkg"))
        }
    };
    match package_project(project, &out_dir) {
        Ok(rep) => {
            println!("Packaged {} -> {}", project.display(), out_dir.display());
            for f in &rep.files {
                println!("  {f}");
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn fmt_time(t_ns: u64) -> String {
    format!("{:.3} ms", t_ns as f64 / NS_PER_MS as f64)
}

/// Renders `Some(kind)` as ", observed <kind>" and `None` as "" — the
/// fault-note suffix for timeline TX lines.
fn observed(err: &Option<cansimcan::can::errors::CanErrorKind>) -> String {
    match err {
        Some(kind) => format!(", observed {kind}"),
        None => String::new(),
    }
}

/// Lexical `.` cleanup for display/CLI paths (no FS access; `..` is
/// rejected earlier by project validation, so only `.` needs collapsing).
fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        if c == Component::CurDir {
            continue;
        }
        out.push(c);
    }
    if out.as_os_str().is_empty() {
        out.push(".");
    }
    out
}

fn cmd_validate(path: &Path) -> i32 {
    match load_and_validate(path) {
        Some(_) => {
            println!("{}: valid", path.display());
            0
        }
        None => 1,
    }
}

/// Extract CAN sends from an Arduino sketch: detection + sends table on
/// stdout, plus a `messages:` YAML snippet when `--as` names the sending
/// node. Static analysis only — the sketch is never compiled or run.
fn cmd_sketch(path: &Path, dialect: Option<&str>, as_node: Option<&str>) -> i32 {
    use cansimcan::sketch::{extract_can_intent, Constness};
    let source = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("failed to read sketch {}: {e}", path.display());
            return 1;
        }
    };
    let intent = match extract_can_intent(&source, dialect) {
        Ok(intent) => intent,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    println!(
        "{}: detected {}",
        path.display(),
        if intent.detected.is_empty() {
            "(override)".into()
        } else {
            intent.detected.join(", ")
        }
    );
    if intent.sends.is_empty() {
        println!("No CAN sends found (the dialect parser saw no send call-sites).");
        return 0;
    }
    println!();
    println!("line  lib          id       ext  dlc  data");
    for send in &intent.sends {
        let id = match &send.id {
            Constness::Const(id) => format!("{id:#X}"),
            Constness::Dynamic(expr) => format!("?{expr}"),
        };
        let ext = match &send.extended {
            Constness::Const(true) => "ext".into(),
            Constness::Const(false) => "-".into(),
            Constness::Dynamic(expr) => format!("?{expr}"),
        };
        let dlc = match &send.dlc {
            Constness::Const(n) => format!("{n}"),
            Constness::Dynamic(expr) => format!("?{expr}"),
        };
        let data = send
            .data
            .iter()
            .map(|b| match b {
                Constness::Const(byte) => format!("{byte:02X}"),
                Constness::Dynamic(expr) => format!("?{expr}"),
            })
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "{:<5} {:<12} {:<8} {:<4} {:<4} {}",
            send.line, send.library, id, ext, dlc, data
        );
    }
    let Some(node) = as_node else {
        println!();
        println!("Rerun with --as <node> to emit a messages: YAML snippet.");
        return 0;
    };
    let short = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "sketch.ino".into());
    println!();
    println!("messages:");
    for send in &intent.sends {
        let (id, extended, dlc, data) = match (&send.id, &send.extended, &send.dlc) {
            (Constness::Const(id), Constness::Const(extended), Constness::Const(dlc)) => {
                (id, extended, dlc, &send.data)
            }
            _ => {
                println!(
                    "  # TODO line {}: dynamic send skipped (fill example bytes by hand)",
                    send.line
                );
                continue;
            }
        };
        if !data.iter().all(|b| !b.is_dynamic()) {
            println!(
                "  # TODO line {}: dynamic payload skipped (fill example bytes by hand)",
                send.line
            );
            continue;
        }
        let bytes: Vec<u8> = data
            .iter()
            .map(|b| match b {
                Constness::Const(byte) => *byte,
                Constness::Dynamic(_) => 0,
            })
            .collect();
        let bytes = bytes[..(*dlc as usize).min(bytes.len())].to_vec();
        println!("  - sender: {node}");
        println!("    id: {id:#X}");
        println!(
            "    data: [{data}]",
            data = bytes
                .iter()
                .map(|b| format!("0x{b:02X}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if *extended {
            println!("    extended: true");
        }
        println!("    source: {short}:{line}", line = send.line);
    }
    0
}

/// Decode one payload with a DBC file: `canlab dbc vehicle.dbc
/// --id 0x100 --data "00 10 50 0A 00 00 00 00"`.
fn cmd_dbc(dbc_path: &Path, id_text: &str, data_text: &str) -> i32 {
    use cansimcan::dbc::Dbc;
    let dbc = match Dbc::load_from_file(dbc_path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let id_trimmed = id_text.trim();
    let id_hex = if id_trimmed.to_lowercase().starts_with("0x") {
        id_trimmed.to_string()
    } else {
        format!("0x{id_trimmed}")
    };
    let id = match u32::from_str_radix(id_hex.trim_start_matches("0x"), 16) {
        Ok(id) => id,
        Err(_) => {
            eprintln!("bad --id \"{id_text}\": expected hex (e.g. 0x100)");
            return 1;
        }
    };
    let mut data = Vec::new();
    if !data_text.trim().is_empty() {
        for part in data_text.split(|c: char| c == ',' || c.is_whitespace()) {
            if part.is_empty() {
                continue;
            }
            match u8::from_str_radix(part, 16) {
                Ok(b) => data.push(b),
                Err(_) => {
                    eprintln!("bad --data byte \"{part}\": expected hex bytes (e.g. \"00 10 50\")");
                    return 1;
                }
            }
        }
    }
    match dbc.decode(id, &data) {
        Ok(signals) => {
            let msg = dbc.message(id).expect("decoded message exists");
            println!("{:#X} {} ({} signal(s)):", id, msg.name, signals.len());
            for (name, value, unit) in signals {
                if unit.is_empty() {
                    println!("  {name} = {value}");
                } else {
                    println!("  {name} = {value} {unit}");
                }
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn cmd_simulate(path: &Path, export_json: Option<&Path>, run_secs: u64) -> i32 {
    let proj = match load_and_validate(path) {
        Some(p) => p,
        None => return 1,
    };

    // Backend dispatch (§56): one backend kind per run. Mixed virtual +
    // renode time sync is not implemented — explicit error, never silent.
    let mut kinds = proj
        .nodes
        .iter()
        .map(|n| n.backend.as_str())
        .collect::<Vec<_>>();
    kinds.sort();
    kinds.dedup();
    match kinds.as_slice() {
        ["virtual"] => cmd_simulate_virtual(&proj, export_json),
        ["renode"] => cmd_simulate_renode(path, &proj, export_json, run_secs),
        _ => {
            eprintln!(
                "cannot simulate: mixed backends in one run ({}).\n\
                 This build runs either all-virtual or all-renode nodes.\n\
                 Fix: set every node to the same backend.",
                kinds.join(", ")
            );
            1
        }
    }
}

fn cmd_simulate_virtual(proj: &Project, export_json: Option<&Path>) -> i32 {
    print_run_header(proj);

    // Scripted traffic, or the canonical §74 demo frame when the project
    // declares no `messages:` (id ranges pre-validated, so unwraps are safe).
    let script: Vec<(String, CanFrame, Option<WireFault>)> = if proj.messages.is_empty() {
        vec![(
            proj.nodes[0].id.clone(),
            CanFrame::new(
                CanId::new_standard(0x123).unwrap(),
                &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
            )
            .unwrap(),
            None,
        )]
    } else {
        proj.messages
            .iter()
            .map(|m| {
                let id = if m.extended {
                    CanId::new_extended(m.id).unwrap()
                } else {
                    CanId::new_standard(m.id as u16).unwrap()
                };
                (m.sender.clone(), CanFrame::new(id, &m.data).unwrap(), None)
            })
            .collect()
    };
    run_virtual_timeline(proj, &script, "transmitted", export_json, true)
}

/// Replay a recorded event-log trace deterministically (§54, headless
/// slice): every transmitted frame in the trace, in order, through a fresh
/// engine on the given (virtual-only) project. The trace format is the
/// `--export-json` event log; RX deliveries and lifecycle markers are
/// derived, not replayed.
fn cmd_replay(
    project: &Path,
    trace: &Path,
    export_json: Option<&Path>,
    as_node: Option<&str>,
) -> i32 {
    let proj = match load_and_validate(project) {
        Some(p) => p,
        None => return 1,
    };
    if proj.nodes.iter().any(|n| n.backend != "virtual") {
        eprintln!(
            "cannot replay: project {} uses non-virtual backends.\n\
             Replay replays recorded virtual traffic deterministically.\n\
             Fix: replay a virtual project, or run renode firmware live: canlab simulate {}",
            project.display(),
            project.display()
        );
        return 1;
    }
    let raw = match std::fs::read(trace) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("failed to read trace {}: {e}", trace.display());
            return 1;
        }
    };
    // Format by magic bytes, not extension: pcap captures (which carry no
    // sender) replay every packet as `--as`; JSON event logs carry senders.
    let script = if cansimcan::pcap::looks_like_pcap(&raw) {
        let Some(node) = as_node else {
            eprintln!(
                "trace {} is a pcap capture, which carries no sender names.\n\
                 Fix: rerun with --as <node> to attribute every packet (e.g. --as engine_ecu).",
                trace.display()
            );
            return 1;
        };
        if !proj.nodes.iter().any(|n| n.id == node) {
            eprintln!("unknown --as node \"{node}\" (project nodes must exist to route through)");
            return 1;
        }
        match cansimcan::pcap::parse_socketcan_pcap(&raw) {
            Ok(frames) => frames
                .into_iter()
                .map(|f| (node.to_string(), f.frame, None))
                .collect(),
            Err(e) => {
                eprintln!("failed to parse pcap trace {}: {e}", trace.display());
                return 1;
            }
        }
    } else {
        let text = match String::from_utf8(raw) {
            Ok(t) => t,
            Err(e) => {
                eprintln!(
                    "failed to read trace {}: not UTF-8 JSON ({e})",
                    trace.display()
                );
                return 1;
            }
        };
        let events: Vec<SimEvent> = match serde_json::from_str(&text) {
            Ok(e) => e,
            Err(e) => {
                eprintln!(
                    "failed to parse trace {}: expected event-log JSON from `canlab simulate --export-json` ({e})",
                    trace.display()
                );
                return 1;
            }
        };
        extract_replay_script(&events)
    };
    if script.is_empty() {
        println!(
            "Trace {} holds no transmitted frames — nothing to replay.",
            trace.display()
        );
        return 0;
    }
    println!(
        "Replaying {} frame(s) from {}...",
        script.len(),
        trace.display()
    );
    println!();
    print_run_header(&proj);
    run_virtual_timeline(&proj, &script, "replayed", export_json, false)
}

fn print_run_header(proj: &Project) {
    println!("Starting CanLab...");
    println!();
    println!("Bus:");
    for b in &proj.buses {
        println!("  {}", b.id);
        println!("  bitrate: {}", b.bitrate);
    }
    println!();
    println!("Nodes:");
    for n in &proj.nodes {
        println!("  {}", n.id);
    }
    println!();
    println!("Simulation started.");
    println!();
}

/// Shared headless timeline: fresh engine, every node on its own project
/// bus, one §75-style TX/RX line per frame, deterministic timestamps.
/// Project fault policies apply unless `apply_faults` is false (replay
/// reproduces the recorded traffic exactly instead of re-drawing faults).
fn run_virtual_timeline(
    proj: &Project,
    script: &[(String, CanFrame, Option<WireFault>)],
    verb: &str,
    export_json: Option<&Path>,
    apply_faults: bool,
) -> i32 {
    use cansimcan::server::session::fault_rules_from_project;
    let mut engine = Engine::new();
    for b in &proj.buses {
        if let Err(e) = engine.add_bus(CanBusConfig::new(b.id.clone(), b.bitrate).unwrap()) {
            eprintln!("failed to create bus {}: {e}", b.id);
            return 1;
        }
    }
    // Every node on its own project bus (multi-bus topologies route
    // per attachment, like the live session — no cross-bus forwarding).
    let mut node_bus = std::collections::HashMap::new();
    for n in &proj.nodes {
        if let Err(e) = engine.register_node(&n.can.bus, &n.id) {
            eprintln!("failed to register node {}: {e}", n.id);
            return 1;
        }
        node_bus.insert(n.id.clone(), n.can.bus.clone());
    }
    engine.set_fault_rules(fault_rules_from_project(proj));
    engine.set_faults_enabled(apply_faults);
    engine.start();

    let mut tx_count = 0u64;
    let mut rx_count = 0u64;

    for (sender, frame, script_fault) in script {
        let bus = match node_bus.get(sender) {
            Some(b) => b.clone(),
            None => {
                eprintln!("transmission from {sender} failed: unknown node");
                return 1;
            }
        };
        let t = engine.now();
        // Scripted drops (replay) re-drive explicitly; everything else goes
        // through the policy-aware transmit (rules off during replay).
        let outcome = match script_fault {
            Some(fault) => engine
                .transmit_with_fault(&bus, sender, frame.clone(), *fault)
                .map(|o| (o.receivers, Some(o.fault), o.error)),
            None => engine
                .transmit(&bus, sender, frame.clone())
                .map(|o| (o.receivers, o.fault, o.error)),
        };
        match outcome {
            Ok((receivers, fault, error)) => {
                // Policy-fired and scripted faults mark the TX line (drops
                // and errored frames print no RX lines, but the TX line
                // alone would read as a clean transmission).
                let fault_note = match (&fault, &error) {
                    (None, _) => String::new(),
                    (Some(WireFault::DropFrame), _) => " [dropped, drove nothing]".to_string(),
                    (Some(WireFault::FlipBit(offset)), err) => {
                        format!(" [fault: wire bit {offset} flipped{}]", observed(err))
                    }
                    (Some(WireFault::CorruptCrc), err) => {
                        format!(" [fault: CRC corrupted{}]", observed(err))
                    }
                };
                println!(
                    "[{}] {sender} TX {} [{}]{}",
                    fmt_time(t),
                    frame.id,
                    frame.data_hex(),
                    fault_note
                );
                for rx in &receivers {
                    println!(
                        "[{}] {rx} RX {} [{}]",
                        fmt_time(t),
                        frame.id,
                        frame.data_hex()
                    );
                }
                tx_count += 1;
                rx_count += receivers.len() as u64;
            }
            Err(e) => {
                eprintln!("transmission from {sender} failed: {e}");
                return 1;
            }
        }
    }

    engine.stop();
    println!();
    println!("Simulation finished: {tx_count} frame(s) {verb}, {rx_count} reception(s).");

    if let Some(out_path) = export_json {
        match serde_json::to_string_pretty(engine.events()) {
            Ok(json) => {
                if let Err(e) = std::fs::write(out_path, json) {
                    eprintln!("failed to write {}: {e}", out_path.display());
                    return 1;
                }
                println!("Event log written to {}", out_path.display());
            }
            Err(e) => {
                eprintln!("failed to serialize event log: {e}");
                return 1;
            }
        }
    }
    0
}

/// Renode run: real firmware executes in the emulator; observed TX frames
/// are replayed through the deterministic engine so the CLI output and the
/// exported event log share the §75 format with virtual runs. Firmware UART
/// lines are printed verbatim as ground truth — engine delivery lines are
/// logical replay, not a second observation.
fn cmd_simulate_renode(
    project_path: &Path,
    proj: &Project,
    export_json: Option<&Path>,
    run_secs: u64,
) -> i32 {
    use cansimcan::backends::api::McuBackend;
    use cansimcan::backends::api::{BackendError, NodeFirmware, RenodeRunSpec};
    use cansimcan::backends::renode::RenodeBackend;

    println!("Starting CanLab... (backend: renode)");
    println!();
    println!("Bus:");
    for b in &proj.buses {
        println!("  {}", b.id);
        println!("  bitrate: {}", b.bitrate);
    }
    println!();
    println!("Nodes:");
    for n in &proj.nodes {
        println!(
            "  {} [{} <- {}]",
            n.id,
            n.device,
            n.firmware.as_deref().unwrap_or("<no firmware>")
        );
    }
    println!();
    if proj.buses.len() > 1 {
        eprintln!(
            "note: {} buses declared; this build simulates \"{}\" only. \
             Multi-bus routing is deferred (explicit limitation, not silent).",
            proj.buses.len(),
            proj.buses[0].id
        );
    }

    // Firmware paths resolve relative to the project file, not the CWD.
    let project_dir = project_path.parent().unwrap_or(Path::new("."));
    let mut nodes = Vec::new();
    for n in &proj.nodes {
        let fw = match &n.firmware {
            Some(f) => normalize(&project_dir.join(f)),
            None => {
                eprintln!(
                    "node \"{}\": backend \"renode\" needs real firmware, but no firmware path is set.\n\
                     Fix: build it with your own toolchain (copy/paste is fine) and set firmware: ./firmware/<name>.elf",
                    n.id
                );
                return 1;
            }
        };
        nodes.push(NodeFirmware {
            machine: NodeFirmware::machine_name(&n.id),
            node_id: n.id.clone(),
            device: n.device.clone(),
            elf: fw,
        });
    }

    let backend = match RenodeBackend::discover() {
        Ok(b) => b,
        Err(e @ BackendError::NotInstalled { .. }) => {
            eprintln!("{e}");
            return 1;
        }
        Err(e) => {
            eprintln!("backend error: {e}");
            return 1;
        }
    };
    println!(
        "Renode: {} (run budget {run_secs}s)",
        backend.binary.display()
    );
    println!("Running firmware... (wall-clock; deterministic timestamps assigned on replay)");
    println!();

    let spec = RenodeRunSpec {
        bus_id: proj.buses[0].id.clone(),
        bitrate: proj.buses[0].bitrate,
        nodes,
        run_secs,
    };
    let outcome = match backend.run(&spec) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("simulation failed: {e}");
            return 1;
        }
    };

    // Ground truth first: every firmware UART line, verbatim.
    println!("Firmware log:");
    let mut init_failures = 0;
    for u in &outcome.uart {
        println!("  [{}] {}", u.machine, u.message);
        if u.message.contains("INIT FAIL") {
            init_failures += 1;
        }
    }
    if init_failures > 0 {
        eprintln!(
            "warning: {init_failures} node(s) reported CAN INIT FAIL (firmware could not enter bxCAN init mode)."
        );
    }
    println!();

    // Replay observed TX frames through the deterministic engine so the
    // run produces the same timestamped event stream as virtual runs.
    let mut engine = Engine::new();
    if let Err(e) =
        engine.add_bus(CanBusConfig::new(proj.buses[0].id.clone(), proj.buses[0].bitrate).unwrap())
    {
        eprintln!("failed to create bus: {e}");
        return 1;
    }
    let bus = proj.buses[0].id.clone();
    for n in &proj.nodes {
        if let Err(e) = engine.register_node(&bus, &n.id) {
            eprintln!("failed to register node {}: {e}", n.id);
            return 1;
        }
    }
    engine.start();
    println!("Simulation started.");
    println!();
    let mut tx_count = 0u64;
    let rx_reported = outcome.rx_frames.len() as u64;
    // Cross-check: every observed RX should match an observed TX (id+data).
    let tx_set: std::collections::HashSet<String> = outcome
        .tx_frames
        .iter()
        .map(|f| format!("{}|{}", f.frame.id, f.frame.data_hex()))
        .collect();
    let mut orphan_rx = 0u64;
    for rx in &outcome.rx_frames {
        let key = format!("{}|{}", rx.frame.id, rx.frame.data_hex());
        if !tx_set.contains(&key) {
            orphan_rx += 1;
            eprintln!(
                "warning: node \"{}\" reported RX {} [{}] with no matching observed TX.",
                rx.node,
                rx.frame.id,
                rx.frame.data_hex()
            );
        }
    }
    for tx in &outcome.tx_frames {
        let t = engine.now();
        match engine.transmit(&bus, &tx.node, tx.frame.clone()) {
            Ok(out) => {
                println!(
                    "[{}] {} TX {} [{}]",
                    fmt_time(t),
                    tx.node,
                    tx.frame.id,
                    tx.frame.data_hex()
                );
                for rx in &out.receivers {
                    println!(
                        "[{}] {rx} RX {} [{}]",
                        fmt_time(t),
                        tx.frame.id,
                        tx.frame.data_hex()
                    );
                }
                tx_count += 1;
            }
            Err(e) => {
                eprintln!("replay of frame from {} failed: {e}", tx.node);
                return 1;
            }
        }
    }
    engine.stop();
    println!();
    println!(
        "Simulation finished: {tx_count} frame(s) transmitted by firmware, {rx_reported} reception(s) reported by firmware."
    );
    if orphan_rx > 0 {
        eprintln!("warning: {orphan_rx} reported reception(s) had no matching transmission.");
    }
    if tx_count > 0 && rx_reported == 0 {
        eprintln!(
            "note: firmware transmitted but no node reported reception. \
             (A lone node still reports TXOK — delivery requires a second node on the hub.)"
        );
    }

    if let Some(out_path) = export_json {
        match serde_json::to_string_pretty(engine.events()) {
            Ok(json) => {
                if let Err(e) = std::fs::write(out_path, json) {
                    eprintln!("failed to write {}: {e}", out_path.display());
                    return 1;
                }
                println!("Event log written to {}", out_path.display());
            }
            Err(e) => {
                eprintln!("failed to serialize event log: {e}");
                return 1;
            }
        }
    }
    0
}

fn probe_binary(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate.display().to_string());
        }
    }
    None
}

/// Repo-local tool roots, in priority order: `$CANLAB_TOOLS`, then
/// `./.tools` (i.e. run doctor from the repo root). Everything CanLab
/// needs can live here with no root access and no system-wide installs.
fn local_tool_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(dir) = std::env::var_os("CANLAB_TOOLS") {
        roots.push(PathBuf::from(dir));
    }
    roots.push(PathBuf::from(".tools"));
    roots
}

fn probe_local(rel: &str) -> Option<PathBuf> {
    local_tool_roots()
        .iter()
        .map(|root| root.join(rel))
        .find(|p| p.is_file())
}

/// Run `<tool> --version`, returning the first output line on success.
fn tool_version(tool: &Path) -> Option<String> {
    let out = std::process::Command::new(tool)
        .arg("--version")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(str::to_string)
}

fn report_tool(label: &str, path_binary: Option<String>, local_rel: &str, purpose: &str) -> bool {
    if let Some(p) = path_binary {
        let ver = tool_version(Path::new(&p))
            .map(|v| format!(" [{v}]"))
            .unwrap_or_default();
        println!("✓ {label} ({p}){ver}");
        return true;
    }
    if let Some(p) = probe_local(local_rel) {
        let ver = tool_version(&p)
            .map(|v| format!(" [{v}]"))
            .unwrap_or_default();
        println!("✓ {label} (repo-local {}){ver}", p.display());
        return true;
    }
    println!("✗ {label} not found ({purpose})");
    false
}

fn cmd_doctor() -> i32 {
    println!("CanLab Doctor");
    println!();
    let mut ok = true;

    match probe_binary("renode") {
        Some(p) => {
            let ver = tool_version(Path::new(&p))
                .map(|v| format!(" [{v}]"))
                .unwrap_or_default();
            println!("✓ Renode ({p}){ver}");
        }
        None if probe_local("renode/renode").is_some() => {
            let p = probe_local("renode/renode").unwrap();
            let ver = tool_version(&p)
                .map(|v| format!(" [{v}]"))
                .unwrap_or_default();
            println!("✓ Renode (repo-local {}){ver}", p.display());
        }
        None => {
            println!("✗ Renode not found on PATH or .tools/renode (needed for Phase 4 real-firmware runs)");
            ok = false;
        }
    }
    // The Renode launcher is a script that execs `dotnet` itself: a present
    // but unrunnable Renode (no runtime) used to fail mid-run. discover()
    // now fails fast on this; doctor reports it the same way.
    match cansimcan::backends::renode::dotnet_runtime_version() {
        Some(v) => println!("✓ dotnet runtime {v} (needed by the Renode launcher)"),
        None => {
            println!(
                "✗ dotnet runtime not found on PATH or .tools/dotnet (Renode runs fail without it)"
            );
            println!("  Fix: install the .NET 8+ runtime repo-locally — one-liner in docs/getting-started.md");
            ok = false;
        }
    }
    if !report_tool(
        "arm-none-eabi-gcc",
        probe_binary("arm-none-eabi-gcc"),
        "arm-gcc/bin/arm-none-eabi-gcc",
        "needed to build STM32 firmware",
    ) {
        ok = false;
    }
    // AVR toolchain: Renode has no AVR core, so Arduino Uno firmware stays
    // on virtual nodes for now (Phase 7 decision); report but don't fail.
    match probe_binary("avr-gcc") {
        Some(p) => println!("✓ avr-gcc ({p})"),
        None => {
            println!("- avr-gcc not found (optional; Arduino builds stay virtual until Phase 7)")
        }
    }
    if Path::new("/sys/class/net/can0").exists() || Path::new("/sys/class/net/vcan0").exists() {
        println!("✓ SocketCAN interface present");
    } else {
        println!("- SocketCAN unavailable (optional; needed for Phase 13 bridging only)");
    }

    println!();
    if ok {
        println!("System is ready for simulated CAN networks.");
    } else {
        println!("Virtual (headless) simulation works now; install the missing tools for firmware builds / Renode runs.");
    }
    0
}
