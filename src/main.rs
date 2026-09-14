//! `canlab` CLI (PROMPT.md §46).
//!
//! - `canlab new <name>` — scaffold a portable project directory (§48)
//! - `canlab simulate <project>` — headless deterministic run, no GUI (§45)
//! - `canlab validate <project>` — schema + safety checks (§21)
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
    },
    /// Validate a project file without simulating.
    Validate { project: PathBuf },
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
        } => cmd_simulate(&project, export_json.as_deref()),
        Commands::Validate { project } => cmd_validate(&project),
        Commands::Doctor => cmd_doctor(),
        Commands::Open { project } => {
            eprintln!(
                "cannot open {}: the visual editor is not implemented yet (Phase 5).\n\
                 Use `canlab simulate {}` for headless runs or `canlab validate {}` to check the project.",
                project.display(),
                project.display(),
                project.display()
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
    let rep = validate_project(&proj);
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

fn cmd_validate(path: &Path) -> i32 {
    match load_and_validate(path) {
        Some(_) => {
            println!("{}: valid", path.display());
            0
        }
        None => 1,
    }
}

fn cmd_simulate(path: &Path, export_json: Option<&Path>) -> i32 {
    let proj = match load_and_validate(path) {
        Some(p) => p,
        None => return 1,
    };

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
