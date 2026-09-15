//! Renode backend: supervised emulator runs for real STM32F103 firmware.
//!
//! Three jobs, kept separate so each is testable without the emulator:
//!
//! 1. [`resolve_binary`] — find `renode` on PATH or repo-local
//!    (`.tools/renode`, `$CANLAB_TOOLS`); actionable error otherwise.
//! 2. [`generate_resc`] — pure function `RenodeRunSpec -> .resc text`
//!    (platform + STMCAN overlay + CAN hub + ELF per node + sync settings,
//!    per the Phase 4 spike findings in `docs/phase4-spike.md`).
//! 3. [`RenodeBackend::run`] — write the script to a unique temp dir, spawn
//!    the child with piped output, drain pipes on threads (no pipe-buffer
//!    deadlock), enforce the wall-clock timeout, kill on timeout or drop
//!    (never orphaned, §19), parse UART lines into [`BackendOutcome`].
//!
//! Device support is explicit (§72): `stm32f103` executes; everything else
//! fails with a named reason instead of pretending.

use super::api::{
    BackendError, BackendOutcome, FrameDir, McuBackend, NodeFirmware, ObservedFrame, ObservedUart,
    RenodeRunSpec,
};
use crate::project::Project;
use std::io::Read;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `CAN.STMCAN` overlay for STM32F103 (stock `stm32f103.repl` ships no CAN).
/// Base `0x40006400`, IRQs TX=19/RX0=20/RX1=21/SCE=22 — mirrors Renode's
/// own `stm32f4.repl` binding and the F103 reference manual.
pub const STM32F103_CAN_OVERLAY: &str =
    "can1: CAN.STMCAN @ sysbus <0x40006400, +0x400> { [0-3] -> nvic@[19-22] }";

/// Only device the Renode backend executes today. Arduino (AVR core) and
/// Teensy (no FlexCAN model in Renode) are named rejections, not TODOs.
pub const RENODE_SUPPORTED_DEVICE: &str = "stm32f103";

pub struct RenodeBackend {
    pub binary: PathBuf,
    /// Resolved `dotnet` host the Renode launcher needs. Carried into the
    /// emulator child's PATH at spawn: discovering dotnet via `.tools/`
    /// must imply Renode can use it, otherwise `discover()` passes and the
    /// child dies with "dotnet not found".
    pub dotnet: PathBuf,
}

impl RenodeBackend {
    /// Locate the emulator *and* the .NET runtime its launcher needs, or
    /// explain how to install whichever is missing (§68).
    pub fn discover() -> Result<Self, BackendError> {
        let binary = resolve_binary()?;
        let dotnet = resolve_dotnet()?;
        Ok(RenodeBackend { binary, dotnet })
    }

    /// Check one node up front: supported device + present, ELF firmware.
    /// Called before spawning anything so config errors fail fast.
    pub fn check_node(node: &NodeFirmware) -> Result<(), BackendError> {
        if node.device != RENODE_SUPPORTED_DEVICE {
            return Err(BackendError::UnsupportedDevice {
                device: node.device.clone(),
                node: node.node_id.clone(),
                backend: "renode".into(),
                hint: device_hint(&node.device),
            });
        }
        if !node.elf.is_file() {
            return Err(BackendError::FirmwareMissing {
                node: node.node_id.clone(),
                path: node.elf.display().to_string(),
            });
        }
        let mut magic = [0u8; 4];
        let mut f = std::fs::File::open(&node.elf).map_err(|e| BackendError::Io(e.to_string()))?;
        f.read_exact(&mut magic)
            .map_err(|e| BackendError::Io(e.to_string()))?;
        if magic != [0x7F, b'E', b'L', b'F'] {
            return Err(BackendError::FirmwareNotElf {
                node: node.node_id.clone(),
                path: node.elf.display().to_string(),
            });
        }
        Ok(())
    }
}

fn device_hint(device: &str) -> String {
    match device {
        "arduino_uno" => "Arduino Uno runs on an AVR core Renode does not model; its CAN path (ATmega328P SPI -> MCP2515) is Phase 7. Use backend \"virtual\" for headless runs until then.".into(),
        "teensy41" => "Teensy 4.1 / FlexCAN has no executing backend yet (Phase 8): Renode's i.MX RT1062 platform only labels the FlexCAN regions, it models no FlexCAN peripheral. Recognized but not emulated — never silently as an STM32.".into(),
        _ => format!("Known devices: {RENODE_SUPPORTED_DEVICE} (renode), stm32f103 / arduino_uno / teensy41 (virtual)."),
    }
}

impl McuBackend for RenodeBackend {
    fn name(&self) -> &'static str {
        "renode"
    }

    fn run(&self, spec: &RenodeRunSpec) -> Result<BackendOutcome, BackendError> {
        let never = AtomicBool::new(false);
        self.run_cancelable(spec, &never)
    }
}

impl RenodeBackend {
    /// Supervised run that aborts early when `cancel` trips (WS job
    /// cancellation): graceful emulator shutdown, then
    /// [`BackendError::Cancelled`] — partial observations are discarded,
    /// never presented as a complete run.
    pub fn run_cancelable(
        &self,
        spec: &RenodeRunSpec,
        cancel: &AtomicBool,
    ) -> Result<BackendOutcome, BackendError> {
        for node in &spec.nodes {
            Self::check_node(node)?;
        }
        if spec.nodes.is_empty() {
            return Err(BackendError::Io(
                "renode run needs at least one node".into(),
            ));
        }
        let script = generate_resc(spec);
        let workdir = create_workdir().map_err(|e| BackendError::Io(e.to_string()))?;
        let resc_path = workdir.join("canlab.resc");
        std::fs::write(&resc_path, script).map_err(|e| BackendError::Io(e.to_string()))?;

        let output = run_supervised(
            &self.binary,
            &self.dotnet,
            &["--disable-xwt", "--port", "0"],
            &[format!("include @{}", resc_path.display()), "start".into()],
            Duration::from_secs(spec.run_secs.max(1)),
            cancel,
        )?;
        // Best-effort cleanup; the log is already captured in `output`.
        let _ = std::fs::remove_dir_all(&workdir);
        Ok(collect_outcome(&output, spec))
    }
}

/// Find the Renode binary: `PATH` first, then repo-local roots
/// (`$CANLAB_TOOLS`, `./.tools`). Mirrors `canlab doctor` probing.
pub fn resolve_binary() -> Result<PathBuf, BackendError> {
    if let Some(path) = probe_path("renode") {
        return Ok(path);
    }
    let mut roots = Vec::new();
    if let Some(dir) = std::env::var_os("CANLAB_TOOLS") {
        roots.push(PathBuf::from(dir));
    }
    roots.push(PathBuf::from(".tools"));
    for root in &roots {
        let candidate = root.join("renode").join("renode");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(BackendError::NotInstalled {
        searched: roots
            .iter()
            .map(|r| r.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
    })
}

fn probe_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Locate a `dotnet` host: `PATH` first, then repo-local roots
/// (`$CANLAB_TOOLS/dotnet/dotnet`, `./.tools/dotnet/dotnet`). The Renode
/// launcher script (`/usr/bin/renode`) execs `dotnet` itself, so a missing
/// runtime used to surface as a cryptic child-process EarlyExit — check it
/// up front instead.
fn find_dotnet() -> Option<PathBuf> {
    if let Some(path) = probe_path("dotnet") {
        return Some(path);
    }
    let mut roots = Vec::new();
    if let Some(dir) = std::env::var_os("CANLAB_TOOLS") {
        roots.push(PathBuf::from(dir));
    }
    roots.push(PathBuf::from(".tools"));
    roots
        .iter()
        .map(|root| root.join("dotnet").join("dotnet"))
        .find(|p| p.is_file())
}

/// Highest `Microsoft.NETCore.App` major version reported by
/// `dotnet --list-runtimes`, e.g. `Some("8.0.31")`. `None` when the host is
/// absent, fails, or reports no usable runtime.
pub fn dotnet_runtime_version() -> Option<String> {
    let dotnet = find_dotnet()?;
    runtime_version_of(&dotnet)
}

/// Newest `Microsoft.NETCore.App` version one `dotnet` host reports.
fn runtime_version_of(dotnet: &Path) -> Option<String> {
    let out = Command::new(dotnet).arg("--list-runtimes").output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_runtime_version(&String::from_utf8_lossy(&out.stdout))
}

/// Parse `--list-runtimes` output for the newest
/// `Microsoft.NETCore.App <major>.<minor>.<patch>` line. Pure (no process),
/// so unit tests cover the version policy without a .NET install.
fn parse_runtime_version(output: &str) -> Option<String> {
    let mut best: Option<(u64, String)> = None;
    for line in output.lines() {
        let Some(rest) = line.strip_prefix("Microsoft.NETCore.App ") else {
            continue;
        };
        let version = rest.split_whitespace().next()?;
        let major: u64 = version.split('.').next()?.parse().ok()?;
        if best.as_ref().map(|(m, _)| major > *m).unwrap_or(true) {
            best = Some((major, version.into()));
        }
    }
    best.map(|(_, v)| v)
}

/// Renode 1.17 targets `net8.0` with `rollForward: Major`, so any runtime
/// major >= 8 runs it. Older (or absent) runtimes fail here with a fix,
/// not mid-run inside the emulator child.
fn resolve_dotnet() -> Result<PathBuf, BackendError> {
    let dotnet = find_dotnet().ok_or(BackendError::DotnetMissing)?;
    let version = runtime_version_of(&dotnet).ok_or(BackendError::DotnetMissing)?;
    if runtime_usable(&version) {
        Ok(dotnet)
    } else {
        Err(BackendError::DotnetTooOld { found: version })
    }
}

/// Version policy, pure for tests: Renode 1.17 targets `net8.0` with
/// `rollForward: Major`, so any runtime major >= 8 runs it.
fn runtime_usable(version: &str) -> bool {
    version
        .split('.')
        .next()
        .and_then(|m| m.parse::<u64>().ok())
        .map(|major| major >= 8)
        .unwrap_or(false)
}

/// Build a [`RenodeRunSpec`] from an already-validated [`Project`].
///
/// Shared by the headless CLI and the WebSocket job table so both enforce
/// the same contract: all nodes on `backend: renode` (mixed projects fail
/// with [`BackendError::MixedBackends` — cross-backend time sync is not
/// implemented), first declared bus (CLI parity: multi-bus routing is
/// deferred, §72), firmware resolved relative to the project file, and
/// every node pre-checked ([`RenodeBackend::check_node`]) so config errors
/// fail before any emulator spawns.
pub fn spec_from_project(
    project: &Project,
    project_path: &Path,
    run_secs: u64,
) -> Result<RenodeRunSpec, BackendError> {
    let mut kinds: Vec<&str> = project.nodes.iter().map(|n| n.backend.as_str()).collect();
    kinds.sort();
    kinds.dedup();
    if kinds != ["renode"] {
        return Err(BackendError::MixedBackends {
            found: kinds.join(", "),
        });
    }
    let bus = project
        .buses
        .first()
        .ok_or_else(|| BackendError::Io("renode run needs at least one bus".into()))?;
    let project_dir = project_path.parent().unwrap_or(Path::new("."));
    let mut nodes = Vec::new();
    for n in &project.nodes {
        let fw = match &n.firmware {
            Some(f) => join_project_relative(project_dir, f),
            None => {
                return Err(BackendError::FirmwareMissing {
                    node: n.id.clone(),
                    path: "<no firmware path set>".into(),
                });
            }
        };
        let node = NodeFirmware {
            machine: NodeFirmware::machine_name(&n.id),
            node_id: n.id.clone(),
            device: n.device.clone(),
            elf: fw,
        };
        RenodeBackend::check_node(&node)?;
        nodes.push(node);
    }
    if nodes.is_empty() {
        return Err(BackendError::Io(
            "renode run needs at least one node".into(),
        ));
    }
    Ok(RenodeRunSpec {
        bus_id: bus.id.clone(),
        bitrate: bus.bitrate,
        nodes,
        run_secs: run_secs.max(1),
    })
}

/// Join a project-relative firmware path onto its project directory,
/// collapsing lexical `.` (no FS access; `..` is rejected earlier by
/// project validation, so only `.` needs collapsing).
fn join_project_relative(base: &Path, rel: &str) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in base.join(rel).components() {
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

/// Pure `.resc` generator: one machine per node, all CAN1s on one hub.
///
/// Renode scripting rules baked in here come from the spike
/// (`docs/phase4-spike.md` finding 5): `#` comments, `sysbus LoadELF`,
/// literal paths with the `@` file prefix, per-machine
/// `connector Connect`, global quantum + serial execution for
/// multi-machine sync. (`@$VAR` and `@...` inside `$var?=` defaults do
/// not resolve — the generator only emits literals and bare `$ORIGIN`
/// defaults, both verified against Renode 1.17.)
pub fn generate_resc(spec: &RenodeRunSpec) -> String {
    let mut s = String::new();
    s.push_str("# Generated by CanLab (backends::renode). Do not hand-edit;\n");
    s.push_str("# regenerate via `canlab simulate`. See docs/phase4-spike.md.\n");
    s.push_str("using sysbus\n\n");
    s.push_str("emulation CreateCANHub \"canHub\" False\n");
    for node in &spec.nodes {
        s.push_str(&format!(
            "\n# --- node: {} (device {}) ---\n",
            node.node_id, node.device
        ));
        s.push_str(&format!("mach create \"{}\"\n", node.machine));
        s.push_str("machine LoadPlatformDescription @platforms/cpus/stm32f103.repl\n");
        s.push_str(&format!(
            "machine LoadPlatformDescriptionFromString \"{}\"\n",
            STM32F103_CAN_OVERLAY
        ));
        // Literal absolute paths need the `@` file prefix (proven in the
        // spike); `$VAR`/`$ORIGIN` forms are used bare. We emit literals,
        // so prefix them. (`@$VAR` does not resolve — never emit that.)
        s.push_str(&format!("sysbus LoadELF @{}\n", node.elf.display()));
        s.push_str("connector Connect sysbus.can1 canHub\n");
        s.push_str("showAnalyzer usart2\n");
    }
    s.push_str("\nemulation SetGlobalQuantum \"0.000025\"\n");
    s.push_str("emulation SetGlobalSerialExecution True\n");
    s
}

/// PATH for the emulator child: the resolved dotnet host's directory first,
/// then the inherited PATH. Pure over `current` (no env reads) so unit
/// tests cover it without mutating the process environment.
fn child_path(dotnet: &Path, current: Option<&std::ffi::OsStr>) -> Option<std::ffi::OsString> {
    let dir = dotnet.parent()?;
    let mut paths = vec![dir.as_os_str().to_owned()];
    if let Some(path) = current {
        paths.extend(std::env::split_paths(path).map(|p| p.into_os_string()));
    }
    std::env::join_paths(paths).ok()
}

// Raw process-group primitives. Declared here (instead of the `libc`
// crate) so the emulator backend adds no dependency: both calls link
// against the system libc already present, and the backend only runs on
// Unix in practice (Renode ships Linux/macOS builds; Windows falls back
// to plain child kill — see `shutdown`).
#[cfg(unix)]
extern "C" {
    fn setsid() -> i32;
    fn killpg(pgrp: i32, sig: i32) -> i32;
}

/// SIGKILL number (signal.h). Passed to [`killpg`] via the raw FFI above.
#[cfg(unix)]
const SIGKILL_RAW: i32 = 9;

/// Unique scratch dir per run (no hard-coded temp paths, §18).
fn create_workdir() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "canlab-renode-{}-{}",
        std::process::id(),
        next_nonce()
    ));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn next_nonce() -> u64 {
    use std::sync::atomic::AtomicU64;
    static NONCE: AtomicU64 = AtomicU64::new(0);
    // Mix in nanos so concurrent CLI invocations (different pids covered
    // above; same pid impossible) still get unique dirs.
    NONCE.fetch_add(1, Ordering::Relaxed)
        ^ (std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos() as u64)
            .unwrap_or(0))
}

/// Spawn Renode with piped output, drain pipes on threads, enforce the
/// wall-clock budget, then shut down.
///
/// `extra_e` commands are passed as separate `-e` flags (one monitor
/// command per flag; `;`-joined assignments do not tokenize).
///
/// Shutdown order (PROMPT.md §19: clean shutdown, never orphaned):
/// 1. If the child exits on its own (finite firmware, script error), reap
///    it: success returns the log, nonzero returns [`BackendError::EarlyExit`].
/// 2. When the budget elapses — the normal end, since firmware loops
///    forever — ask for a graceful `quit` over the telnet monitor, wait
///    briefly, then fall back to kill. The child is always reaped.
fn run_supervised(
    binary: &Path,
    dotnet: &Path,
    args: &[&str],
    extra_e: &[String],
    budget: Duration,
    cancel: &AtomicBool,
) -> Result<String, BackendError> {
    let mut cmd = Command::new(binary);
    cmd.args(args);
    for e in extra_e {
        cmd.arg("-e").arg(e);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    // New session + process group for the emulator subtree: launcher
    // scripts fork grandchildren (bash `renode` -> `dotnet Renode.dll`)
    // that inherit the pipes, so killing only the direct child orphans
    // them and hangs the pipe drainers forever (user-hit hang on
    // cancel-during-boot). The group dies as one on killpg.
    #[cfg(unix)]
    unsafe {
        cmd.pre_exec(|| {
            // SAFETY: setsid(2) is async-signal-safe, which is all
            // pre_exec (post-fork, pre-exec) context permits. The child
            // is never a group leader here (fresh pid), so this succeeds;
            // failure aborts the spawn loudly via SpawnFailed.
            if setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    // The Renode launcher execs `dotnet` itself: give the child the
    // resolved host's directory first on PATH, so repo-local runtimes work
    // without the user exporting anything.
    if let Some(path) = child_path(dotnet, std::env::var_os("PATH").as_deref()) {
        cmd.env("PATH", path);
    }
    // Run from the Renode install dir so `@platforms/...` resolves, exactly
    // like the documented manual runs.
    if let Some(dir) = binary.parent() {
        // Portable layout: <root>/renode binary with platforms/ beside it.
        let root = dir;
        if root.join("platforms").is_dir() {
            cmd.current_dir(root);
        } else if let Some(parent) = root.parent() {
            if parent.join("platforms").is_dir() {
                cmd.current_dir(parent);
            }
        }
    }
    let mut child: Child = cmd.spawn().map_err(|e| BackendError::SpawnFailed {
        binary: binary.display().to_string(),
        cause: e.to_string(),
    })?;

    // Drain pipes on threads so a chatty emulation can never block on a
    // full pipe buffer while we wait for the timeout.
    let stdout_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let stderr_buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let mut threads = Vec::new();
    if let Some(out) = child.stdout.take() {
        let buf = Arc::clone(&stdout_buf);
        threads.push(std::thread::spawn(move || {
            let mut src = out;
            let mut tmp = [0u8; 8192];
            loop {
                match src.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.lock().unwrap().extend_from_slice(&tmp[..n]),
                }
            }
        }));
    }
    if let Some(err) = child.stderr.take() {
        let buf = Arc::clone(&stderr_buf);
        threads.push(std::thread::spawn(move || {
            let mut src = err;
            let mut tmp = [0u8; 8192];
            loop {
                match src.read(&mut tmp) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.lock().unwrap().extend_from_slice(&tmp[..n]),
                }
            }
        }));
    }

    let start = Instant::now();
    loop {
        match child
            .try_wait()
            .map_err(|e| BackendError::Io(e.to_string()))?
        {
            Some(status) => {
                // Natural exit: reap, join drainers, collect output.
                let _ = child.wait();
                join_bounded(threads, Duration::from_secs(5));
                return finish(status, &stdout_buf, &stderr_buf);
            }
            None => {
                if cancel.load(Ordering::Relaxed) {
                    // User abort: same graceful shutdown as budget end, but
                    // the partial log is discarded — Cancelled, not Done.
                    let _ = shutdown(&mut child, threads, &stdout_buf, &stderr_buf);
                    return Err(BackendError::Cancelled);
                }
                if start.elapsed() >= budget {
                    return shutdown(&mut child, threads, &stdout_buf, &stderr_buf);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

/// Collect drained output and map a natural exit to log-or-error.
fn finish(
    status: std::process::ExitStatus,
    stdout_buf: &Arc<Mutex<Vec<u8>>>,
    stderr_buf: &Arc<Mutex<Vec<u8>>>,
) -> Result<String, BackendError> {
    let log = drain_to_string(stdout_buf, stderr_buf);
    if status.success() {
        Ok(log)
    } else {
        // Renode exits nonzero on script errors (bad platform, missing
        // ELF…). Surface the tail, not a bare status code.
        Err(BackendError::EarlyExit {
            status: status.to_string(),
            tail: tail_of(&log, 25),
        })
    }
}

/// Budget elapsed: graceful `quit` via the telnet monitor if we learned its
/// port, brief grace period, then kill. The child is always reaped — no
/// orphaned emulator processes (§19).
///
/// Kill semantics: the emulator runs in its own process group (see spawn),
/// so `killpg` takes launcher grandchildren with it. Pipe drainers are
/// joined with a bound — a surviving writer can delay but never hang us.
fn shutdown(
    child: &mut Child,
    threads: Vec<std::thread::JoinHandle<()>>,
    stdout_buf: &Arc<Mutex<Vec<u8>>>,
    stderr_buf: &Arc<Mutex<Vec<u8>>>,
) -> Result<String, BackendError> {
    if let Some(port) = monitor_port(stdout_buf) {
        if send_quit(port) {
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                match child
                    .try_wait()
                    .map_err(|e| BackendError::Io(e.to_string()))?
                {
                    Some(status) => {
                        let _ = child.wait();
                        join_bounded(threads, Duration::from_secs(5));
                        return finish(status, stdout_buf, stderr_buf);
                    }
                    None => std::thread::sleep(Duration::from_millis(100)),
                }
            }
        }
    }
    kill_subtree(child);
    let status = child.wait().map_err(|e| BackendError::Io(e.to_string()))?;
    join_bounded(threads, Duration::from_secs(5));
    // Killed after a full budget of observations: that IS the run result.
    // A nonzero-after-kill status is expected (SIGKILL), not an error.
    let _ = status;
    Ok(drain_to_string(stdout_buf, stderr_buf))
}

/// Kill the emulator subtree. Unix: the whole process group (the child is
/// its leader via setsid at spawn). Elsewhere: the direct child only.
fn kill_subtree(child: &mut Child) {
    #[cfg(unix)]
    {
        // ESRCH (group already gone) just means graceful quit won the race.
        unsafe {
            killpg(child.id() as i32, SIGKILL_RAW);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

/// Join pipe drainers, waiting at most `wait` per thread. Leftovers are
/// detached (leaked handles, bounded buffers): after a group kill their
/// pipes hit EOF promptly, so in practice nothing is left behind — but a
/// surviving writer can delay us, never hang us.
fn join_bounded(threads: Vec<std::thread::JoinHandle<()>>, wait: Duration) {
    for t in threads {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = t.join();
            let _ = tx.send(());
        });
        let _ = rx.recv_timeout(wait);
    }
}

fn drain_to_string(stdout_buf: &Arc<Mutex<Vec<u8>>>, stderr_buf: &Arc<Mutex<Vec<u8>>>) -> String {
    let mut log = String::from_utf8_lossy(&stdout_buf.lock().unwrap()).into_owned();
    let err = String::from_utf8_lossy(&stderr_buf.lock().unwrap()).into_owned();
    if !err.is_empty() {
        log.push_str("\n--- stderr ---\n");
        log.push_str(&err);
    }
    log
}

fn tail_of(log: &str, n: usize) -> String {
    log.lines()
        .rev()
        .take(n)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Scrape the telnet monitor port from Renode's stdout
/// (`Monitor available in telnet mode on port N`).
fn monitor_port(stdout_buf: &Arc<Mutex<Vec<u8>>>) -> Option<u16> {
    let text = String::from_utf8_lossy(&stdout_buf.lock().unwrap()).into_owned();
    let marker = "telnet mode on port ";
    let idx = text.rfind(marker)?;
    text[idx + marker.len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// Best-effort graceful quit: connect, send `quit`, close. Returns whether
/// the command was delivered (the caller still verifies process exit).
fn send_quit(port: u16) -> bool {
    use std::io::Write as _;
    let addr = format!("127.0.0.1:{port}");
    let Ok(stream) = std::net::TcpStream::connect_timeout(
        &addr.parse().expect("loopback addr parses"),
        Duration::from_secs(2),
    ) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut stream = stream;
    stream.write_all(b"quit\n").is_ok()
}

/// Parse the emulator log into UART observations + CAN frames.
///
/// `machine -> node` comes from the spec (we generated the machine names).
pub fn collect_outcome(log: &str, spec: &RenodeRunSpec) -> BackendOutcome {
    use super::api::{parse_can_line, parse_uart_line};
    let by_machine: std::collections::HashMap<&str, &str> = spec
        .nodes
        .iter()
        .map(|n| (n.machine.as_str(), n.node_id.as_str()))
        .collect();
    let mut uart = Vec::new();
    let mut tx_frames = Vec::new();
    let mut rx_frames = Vec::new();
    for line in log.lines() {
        let Some(u) = parse_uart_line(line) else {
            continue;
        };
        let node = by_machine
            .get(u.machine.as_str())
            .copied()
            .unwrap_or(u.machine.as_str());
        if let Some(f) = parse_can_line(&u.message, node) {
            let dir = f.dir;
            let obs = ObservedFrame {
                node: node.into(),
                dir,
                frame: f.frame,
            };
            match dir {
                FrameDir::Tx => tx_frames.push(obs),
                FrameDir::Rx => rx_frames.push(obs),
            }
        }
        uart.push(ObservedUart {
            machine: u.machine,
            message: u.message,
        });
    }
    BackendOutcome {
        uart,
        tx_frames,
        rx_frames,
        raw_log: log.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::api::NodeFirmware;
    use super::*;

    fn spec_two_nodes() -> RenodeRunSpec {
        RenodeRunSpec {
            bus_id: "vehicle_bus".into(),
            bitrate: 500_000,
            nodes: vec![
                NodeFirmware {
                    node_id: "engine_ecu".into(),
                    machine: NodeFirmware::machine_name("engine_ecu"),
                    device: "stm32f103".into(),
                    elf: PathBuf::from("/tmp/firmware/engine.elf"),
                },
                NodeFirmware {
                    node_id: "dash board!".into(),
                    machine: NodeFirmware::machine_name("dash board!"),
                    device: "stm32f103".into(),
                    elf: PathBuf::from("/tmp/firmware/dash.hex"),
                },
            ],
            run_secs: 90,
        }
    }

    #[test]
    fn resc_has_overlay_hub_machine_per_node_and_sync() {
        let resc = generate_resc(&spec_two_nodes());
        assert!(resc.contains("emulation CreateCANHub \"canHub\" False"));
        assert!(resc.contains(STM32F103_CAN_OVERLAY));
        assert!(resc.contains("mach create \"engine_ecu\""));
        assert!(resc.contains("mach create \"dash_board_\""));
        assert!(resc.contains("sysbus LoadELF @/tmp/firmware/engine.elf"));
        assert!(resc.contains("connector Connect sysbus.can1 canHub"));
        assert!(resc.contains("emulation SetGlobalQuantum \"0.000025\""));
        assert!(resc.contains("emulation SetGlobalSerialExecution True"));
        // No `@$VAR` forms (known Renode hang): LoadELF lines use literal
        // `@/abs/path` or bare `$VAR` paths.
        assert!(!resc.contains("@$"));
    }

    #[test]
    fn check_node_rejects_devices_and_non_elf() {
        let dir = std::env::temp_dir();
        let elf = dir.join("canlab-test-notelf.bin");
        std::fs::write(&elf, b"not an elf").unwrap();
        let bad = NodeFirmware {
            node_id: "n".into(),
            machine: "n".into(),
            device: "arduino_uno".into(),
            elf: elf.clone(),
        };
        let err = RenodeBackend::check_node(&bad).unwrap_err();
        assert!(err.to_string().contains("Phase 7"), "{err}");
        let teensy = NodeFirmware {
            device: "teensy41".into(),
            ..bad.clone()
        };
        let err = RenodeBackend::check_node(&teensy).unwrap_err();
        assert!(err.to_string().contains("Phase 8"), "{err}");
        let notelf = NodeFirmware {
            device: "stm32f103".into(),
            ..bad
        };
        let err = RenodeBackend::check_node(&notelf).unwrap_err();
        assert!(err.to_string().contains("not an ELF"), "{err}");
        let missing = NodeFirmware {
            node_id: "n".into(),
            machine: "n".into(),
            device: "stm32f103".into(),
            elf: dir.join("canlab-test-nope.elf"),
        };
        let err = RenodeBackend::check_node(&missing).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
        let _ = std::fs::remove_file(&elf);
    }

    #[test]
    fn collects_frames_from_spike_log() {
        let log = "[16:11:27.4125] [INFO] tx-node/usart2: [host: 0.18s|virt: 0s] CANLAB TX BOOT\n\
                   [16:11:30.6374] [INFO] tx-node/usart2: [host: 3.4s|virt: 0.4s] CAN TX OK 0x123 [01 02 03 04 05 06 07 08]\n\
                   [16:11:30.6444] [INFO] rx-node/usart2: [host: 3.29s|virt: 0.4s] CAN RX 0x123 [01 02 03 04 05 06 07 08]\n\
                   [16:11:27.3731] [INFO] tx-node: Machine started.\n";
        let spec = RenodeRunSpec {
            bus_id: "b".into(),
            bitrate: 500_000,
            nodes: vec![
                NodeFirmware {
                    node_id: "engine_ecu".into(),
                    machine: "tx-node".into(),
                    device: "stm32f103".into(),
                    elf: PathBuf::from("/x"),
                },
                NodeFirmware {
                    node_id: "dashboard".into(),
                    machine: "rx-node".into(),
                    device: "stm32f103".into(),
                    elf: PathBuf::from("/y"),
                },
            ],
            run_secs: 90,
        };
        let out = collect_outcome(log, &spec);
        assert_eq!(out.uart.len(), 3);
        assert_eq!(out.tx_frames.len(), 1);
        assert_eq!(out.rx_frames.len(), 1);
        assert_eq!(out.tx_frames[0].node, "engine_ecu");
        assert_eq!(out.rx_frames[0].node, "dashboard");
        assert_eq!(out.tx_frames[0].frame, out.rx_frames[0].frame);
    }

    #[test]
    fn workdir_is_unique_and_cleanable() {
        let a = create_workdir().unwrap();
        let b = create_workdir().unwrap();
        assert_ne!(a, b);
        std::fs::remove_dir_all(&a).unwrap();
        std::fs::remove_dir_all(&b).unwrap();
    }

    /// The cancel-during-boot hang, pinned without an emulator: a stub
    /// launcher that forks a grandchild holding stdout (like bash `renode`
    /// -> `dotnet Renode.dll`) and waits. Killing only the direct child
    /// would orphan the grandchild and hang the pipe drainers forever;
    /// the process-group kill must reap everything promptly.
    #[test]
    #[cfg(unix)]
    fn kill_path_reaps_launcher_grandchildren() {
        use std::io::Write as _;
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("canlab-orphan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Unique grandchild identity: copy sleep under a probe name so the
        // orphan check cannot match unrelated processes.
        let probe = dir.join(format!("cansimcan_probe_sleep_{}", std::process::id()));
        std::fs::copy("/bin/sleep", &probe).expect("a sleep binary to copy");
        let stub = dir.join("stub-launcher.sh");
        let mut f = std::fs::File::create(&stub).unwrap();
        writeln!(f, "#!/bin/sh").unwrap();
        writeln!(f, "{} 300 &", probe.display()).unwrap();
        writeln!(f, "echo launcher-ready").unwrap();
        writeln!(f, "wait").unwrap();
        drop(f);
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Pre-tripped cancel aborts on the first supervision tick with no
        // monitor line in sight — straight to the kill path.
        let cancel = AtomicBool::new(true);
        let start = Instant::now();
        let err = run_supervised(
            &stub,
            Path::new("/bin/true"),
            &[],
            &[],
            Duration::from_secs(120),
            &cancel,
        )
        .unwrap_err();
        assert!(matches!(err, BackendError::Cancelled), "{err}");
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "kill path must return promptly, took {:?}",
            start.elapsed()
        );
        // The grandchild must actually be gone (poll: init reaps orphans
        // asynchronously, but a leaked sleeper would persist 300 s).
        let gone = (0..50).any(|_| {
            let out = std::process::Command::new("pgrep")
                .args(["-f", &probe.display().to_string()])
                .output()
                .expect("pgrep must exist for the orphan check");
            if !out.status.success() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
            false
        });
        assert!(gone, "orphaned emulator grandchild survived the kill path");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn child_path_puts_dotnet_first() {
        let dotnet = PathBuf::from("/repo/.tools/dotnet/dotnet");
        let joined = child_path(&dotnet, Some(std::ffi::OsStr::new("/usr/bin:/bin"))).unwrap();
        let mut parts = std::env::split_paths(&joined);
        assert_eq!(
            parts.next().as_deref(),
            Some(Path::new("/repo/.tools/dotnet"))
        );
        assert_eq!(parts.next().as_deref(), Some(Path::new("/usr/bin")));
        // No inherited PATH: the host dir alone still works.
        let alone = child_path(&dotnet, None).unwrap();
        assert_eq!(alone, "/repo/.tools/dotnet");
    }

    fn write_elf(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, [0x7F, b'E', b'L', b'F', 0, 0, 0, 0]).unwrap();
        path
    }

    const RENODE_PROJ: &str = r#"
version: 1
simulation: {mode: deterministic}
buses:
  - {id: vehicle_bus, type: can, bitrate: 500000, fd: false}
nodes:
  - {id: engine_ecu, device: stm32f103, backend: renode, firmware: ./engine.elf, can: {bus: vehicle_bus}}
  - {id: dashboard, device: stm32f103, backend: renode, firmware: ./dash.elf, can: {bus: vehicle_bus}}
"#;

    #[test]
    fn spec_from_project_resolves_and_clamps() {
        let dir = std::env::temp_dir().join(format!("canlab-spec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        write_elf(&dir, "engine.elf");
        write_elf(&dir, "dash.elf");
        let proj = Project::parse(RENODE_PROJ).unwrap();
        let spec = spec_from_project(&proj, &dir.join("p.canlab"), 0).unwrap();
        // run_secs 0 is meaningless (instant shutdown) — clamped to 1.
        assert_eq!(spec.run_secs, 1);
        assert_eq!(spec.bus_id, "vehicle_bus");
        assert_eq!(spec.nodes.len(), 2);
        assert_eq!(spec.nodes[0].machine, "engine_ecu");
        assert!(spec.nodes[0].elf.is_absolute());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn spec_from_project_rejects_mixed_and_missing() {
        let dir = std::env::temp_dir().join(format!("canlab-spec-rej-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // engine.elf exists so the missing-firmware probe reaches dashboard.
        write_elf(&dir, "engine.elf");
        let proj = Project::parse(RENODE_PROJ).unwrap();
        // Missing ELF files fail before any emulator spawns.
        let err = spec_from_project(&proj, &dir.join("p.canlab"), 5).unwrap_err();
        assert!(err.to_string().contains("not found"), "{err}");
        // Mixed backends are never silently time-synced.
        let mixed = RENODE_PROJ.replacen("backend: renode", "backend: virtual", 1);
        let err = spec_from_project(&Project::parse(&mixed).unwrap(), &dir.join("p.canlab"), 5)
            .unwrap_err();
        assert!(matches!(err, BackendError::MixedBackends { .. }), "{err}");
        // A node without firmware fails with its name attached.
        let nofw = RENODE_PROJ.replace(", firmware: ./dash.elf", "");
        let err = spec_from_project(&Project::parse(&nofw).unwrap(), &dir.join("p.canlab"), 5)
            .unwrap_err();
        assert!(err.to_string().contains("dashboard"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parses_dotnet_runtimes_and_picks_newest() {
        let out = concat!(
            "Microsoft.NETCore.App 6.0.25 [/usr/share/dotnet/shared/Microsoft.NETCore.App]\n",
            "Microsoft.NETCore.App 8.0.31 [/home/u/.tools/dotnet/shared/Microsoft.NETCore.App]\n",
            "Microsoft.AspNetCore.App 8.0.31 [/home/u/.tools/dotnet/shared/Microsoft.AspNetCore.App]\n",
        );
        assert_eq!(parse_runtime_version(out).as_deref(), Some("8.0.31"));
        assert_eq!(
            parse_runtime_version("Microsoft.NETCore.App 9.1.0 [/x]\n").as_deref(),
            Some("9.1.0")
        );
        assert!(parse_runtime_version("").is_none());
        assert!(parse_runtime_version("garbage\nMicrosoft.AspNetCore.App 8.0.31 [/x]\n").is_none());
    }

    #[test]
    fn runtime_policy_matches_renode_net8_rollforward_major() {
        assert!(runtime_usable("8.0.31"));
        assert!(runtime_usable("9.0.0"));
        assert!(!runtime_usable("6.0.25"));
        assert!(!runtime_usable("garbage"));
    }

    #[test]
    fn dotnet_errors_are_actionable() {
        let missing = BackendError::DotnetMissing;
        assert!(missing.to_string().contains("dotnet"), "{missing}");
        assert!(missing.to_string().contains(".tools/dotnet"), "{missing}");
        let old = BackendError::DotnetTooOld {
            found: "6.0.25".into(),
        };
        assert!(old.to_string().contains("6.0.25"), "{old}");
        assert!(old.to_string().contains(".NET 8"), "{old}");
    }
}
