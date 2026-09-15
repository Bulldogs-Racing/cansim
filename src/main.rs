//! `canlab` CLI (PROMPT.md §46).
//!
//! - `canlab new <name>` — scaffold a portable project directory (§48)
//! - `canlab simulate <project>` — headless deterministic run, no GUI (§45)
//! - `canlab validate <project>` — schema + safety checks (§21)
//! - `canlab serve [--port N] [--project p]` — local WebSocket API for the
//!   visual editor (§4); the GUI drives this, never the engine directly
//! - `canlab doctor` — environment diagnostics (Renode, toolchains, SocketCAN)

use cansimcan::can::bus::CanBusConfig;
use cansimcan::can::frame::CanFrame;
use cansimcan::can::id::CanId;
use cansimcan::can::timing::NS_PER_MS;
use cansimcan::project::{validate_project, Project};
use cansimcan::simulation::engine::Engine;
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
    /// Serve the local WebSocket API for the visual editor.
    Serve {
        /// Loopback port to listen on.
        #[arg(long, default_value_t = 21011)]
        port: u16,
        /// Project file to preload into the session.
        #[arg(long)]
        project: Option<PathBuf>,
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
        Commands::Simulate {
            project,
            export_json,
            run_secs,
        } => cmd_simulate(&project, export_json.as_deref(), run_secs),
        Commands::Validate { project } => cmd_validate(&project),
        Commands::Serve { port, project } => {
            cansimcan::server::serve::serve(port, project.as_deref())
        }
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

fn fmt_time(t_ns: u64) -> String {
    format!("{:.3} ms", t_ns as f64 / NS_PER_MS as f64)
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

    let mut engine = Engine::new();
    for b in &proj.buses {
        if let Err(e) = engine.add_bus(CanBusConfig::new(b.id.clone(), b.bitrate).unwrap()) {
            eprintln!("failed to create bus {}: {e}", b.id);
            return 1;
        }
    }
    // Phase 3 supports one bus per run; multi-bus topologies validate but
    // need explicit routing (deferred with a clear message, §72).
    if proj.buses.len() > 1 {
        eprintln!(
            "note: {} buses declared; this build simulates \"{}\" only. \
             Multi-bus routing is deferred (explicit limitation, not silent).",
            proj.buses.len(),
            proj.buses[0].id
        );
    }
    let bus = proj.buses[0].id.clone();
    for n in &proj.nodes {
        if let Err(e) = engine.register_node(&bus, &n.id) {
            eprintln!("failed to register node {}: {e}", n.id);
            return 1;
        }
    }
    engine.start();

    let mut tx_count = 0u64;
    let mut rx_count = 0u64;

    if proj.messages.is_empty() {
        // Default demo: first node sends the canonical §74 frame.
        let sender = &proj.nodes[0].id;
        let frame = CanFrame::new(
            CanId::new_standard(0x123).unwrap(),
            &[0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08],
        )
        .unwrap();
        let t = engine.now();
        match engine.transmit(&bus, sender, frame.clone()) {
            Ok(out) => {
                println!(
                    "[{}] {sender} TX {} [{}]",
                    fmt_time(t),
                    frame.id,
                    frame.data_hex()
                );
                for rx in &out.receivers {
                    println!(
                        "[{}] {rx} RX {} [{}]",
                        fmt_time(t),
                        frame.id,
                        frame.data_hex()
                    );
                }
                tx_count += 1;
                rx_count += out.receivers.len() as u64;
            }
            Err(e) => {
                eprintln!("transmission failed: {e}");
                return 1;
            }
        }
    } else {
        for m in &proj.messages {
            let id = if m.extended {
                CanId::new_extended(m.id).unwrap()
            } else {
                CanId::new_standard(m.id as u16).unwrap()
            };
            let frame = CanFrame::new(id, &m.data).unwrap();
            let t = engine.now();
            match engine.transmit(&bus, &m.sender, frame.clone()) {
                Ok(out) => {
                    println!(
                        "[{}] {} TX {} [{}]",
                        fmt_time(t),
                        m.sender,
                        frame.id,
                        frame.data_hex()
                    );
                    for rx in &out.receivers {
                        println!(
                            "[{}] {rx} RX {} [{}]",
                            fmt_time(t),
                            frame.id,
                            frame.data_hex()
                        );
                    }
                    tx_count += 1;
                    rx_count += out.receivers.len() as u64;
                }
                Err(e) => {
                    eprintln!("transmission from {} failed: {e}", m.sender);
                    return 1;
                }
            }
        }
    }

    engine.stop();
    println!();
    println!("Simulation finished: {tx_count} frame(s) transmitted, {rx_count} reception(s).");

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
