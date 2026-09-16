//! `canlab serve`: local WebSocket API for the visual editor.
//!
//! Sync threads only (tungstenite server, no async runtime): one thread per
//! connection, one shared `Mutex<Session>`. Requests are handled
//! sequentially per connection; a slow client cannot corrupt the session.
//!
//! Every request gets exactly one JSON reply. `GetEvents { sinceSeq }` is
//! the analyzer's poll primitive — the GUI polls it and renders new rows.

use super::jobs::{JobRecord, JobState, JobTable};
use super::proto::{parse_client_msg, seq_event, state_name, ClientMsg, JobInfo, ServerMsg};
use super::session::Session;
use crate::backends::renode::{spec_from_project, RenodeBackend};
use crate::project::{validate_project, Project};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use tungstenite::Message;

/// Authoritative server state: one loaded project + engine ([`Session`])
/// plus the background firmware-run table ([`JobTable`]), shared by all
/// connections behind one `Mutex`.
#[derive(Debug)]
pub struct ServerState {
    pub session: Session,
    pub jobs: JobTable,
}

impl ServerState {
    pub fn new(max_jobs: u16) -> Self {
        ServerState {
            session: Session::new(),
            jobs: JobTable::new(max_jobs),
        }
    }
}

/// Serve forever on `127.0.0.1:port`. Returns only on accept failure.
pub fn serve(port: u16, preload: Option<&Path>, max_jobs: u16) -> i32 {
    if max_jobs == 0 {
        eprintln!("--max-jobs must be at least 1 (no Renode job could ever start)");
        return 1;
    }
    let state = Arc::new(Mutex::new(ServerState::new(max_jobs)));
    if let Some(path) = preload {
        match state.lock().unwrap().session.load(path) {
            Ok(()) => println!("Loaded {}", path.display()),
            Err(e) => {
                eprintln!("cannot preload {}: {e}", path.display());
                return 1;
            }
        }
    }
    let addr = format!("127.0.0.1:{port}");
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen on {addr}: {e}");
            return 1;
        }
    };
    println!("CanLab API serving on ws://{addr}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let shared = Arc::clone(&state);
                std::thread::spawn(move || handle_conn(stream, shared));
            }
            Err(e) => {
                eprintln!("accept failed: {e}");
                return 1;
            }
        }
    }
    0
}

/// Bind an ephemeral port for tests; returns the chosen port.
pub fn serve_ephemeral(state: Arc<Mutex<ServerState>>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    let port = listener.local_addr().expect("local addr").port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let shared = Arc::clone(&state);
                    std::thread::spawn(move || handle_conn(stream, shared));
                }
                Err(_) => break,
            }
        }
    });
    port
}

fn handle_conn(stream: std::net::TcpStream, state: Arc<Mutex<ServerState>>) {
    let mut ws = match tungstenite::accept(stream) {
        Ok(ws) => ws,
        Err(_) => return,
    };
    loop {
        let msg = match ws.read() {
            Ok(m) => m,
            Err(_) => break,
        };
        match msg {
            Message::Text(text) => {
                let reply = dispatch(&state, &text);
                let out = serde_json::to_string(&reply)
                    .unwrap_or_else(|_| r#"{"type":"Error","message":"encode failed"}"#.into());
                if ws.send(Message::Text(out.into())).is_err() {
                    break;
                }
            }
            Message::Binary(_) => {
                let reply = ServerMsg::Error {
                    message: "binary frames not supported; send JSON text".into(),
                };
                if ws
                    .send(Message::Text(serde_json::to_string(&reply).unwrap().into()))
                    .is_err()
                {
                    break;
                }
            }
            Message::Ping(data) => {
                if ws.send(Message::Pong(data)).is_err() {
                    break;
                }
            }
            Message::Close(_) => {
                let _ = ws.close(None);
                break;
            }
            _ => {}
        }
    }
}

/// One request in, one reply out. Lock scope covers the whole dispatch so
/// concurrent tabs observe a consistent session.
fn dispatch(state: &Arc<Mutex<ServerState>>, text: &str) -> ServerMsg {
    let req = match parse_client_msg(text) {
        Ok(r) => r,
        Err(message) => return ServerMsg::Error { message },
    };
    let mut s = state.lock().unwrap();
    match req {
        ClientMsg::Ping => ServerMsg::Pong,
        ClientMsg::Load { path } => match s.session.load(Path::new(&path)) {
            Ok(()) => status_of(&s.session),
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::Start => match s.session.start() {
            Ok(sum) => ServerMsg::RunSummary {
                transmitted: sum.transmitted,
                received: sum.received,
                nextSeq: s.session.events().len(),
            },
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::Pause => ok_or_error(s.session.pause().map(|()| status_of(&s.session))),
        ClientMsg::Resume => ok_or_error(s.session.resume().map(|()| status_of(&s.session))),
        ClientMsg::Stop => ok_or_error(s.session.stop().map(|()| status_of(&s.session))),
        ClientMsg::Reset => ok_or_error(s.session.reset().map(|()| status_of(&s.session))),
        ClientMsg::Step { deltaNs } => match s.session.step(deltaNs) {
            Ok(now) => ServerMsg::Stepped {
                nowNs: now,
                nextSeq: s.session.events().len(),
            },
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::Inject {
            sender,
            id,
            extended,
            data,
        } => match s.session.inject(&sender, id, extended, &data) {
            Ok(receivers) => ServerMsg::Injected {
                receivers,
                nextSeq: s.session.events().len(),
            },
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::Arbitrate { frames } => match arbitrate_frames(&mut s.session, &frames) {
            Ok(rep) => ServerMsg::Arbitrated {
                winner: rep.winner,
                winnerId: rep.winner_id,
                winnerExtended: rep.winner_extended,
                losers: rep.losers,
                receivers: rep.receivers,
                nextSeq: s.session.events().len(),
            },
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::InspectFrame {
            id,
            extended,
            data,
            remote,
            dlc,
        } => match inspect_frame(id, extended, &data, remote, dlc) {
            Ok(rep) => rep,
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::DecodeFrame { dbc, id, data } => match decode_frame(&dbc, id, &data) {
            Ok(rep) => rep,
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::InjectFault {
            sender,
            id,
            extended,
            data,
            fault,
        } => match s.session.inject_fault(&sender, id, extended, &data, fault) {
            Ok(rep) => ServerMsg::FaultInjected {
                error: rep.error,
                receivers: rep.receivers,
                nextSeq: s.session.events().len(),
            },
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::GetEvents { sinceSeq } => {
            let log = s.session.events();
            let from = sinceSeq.min(log.len());
            ServerMsg::Events {
                events: log
                    .iter()
                    .enumerate()
                    .skip(from)
                    .map(|(i, e)| seq_event(i, e))
                    .collect(),
                nextSeq: log.len(),
            }
        }
        ClientMsg::NewProject => {
            s.session.new_project();
            status_of(&s.session)
        }
        ClientMsg::AddBus { id, bitrate } => ok_or_error(
            s.session
                .add_bus(&id, bitrate)
                .map(|()| status_of(&s.session)),
        ),
        ClientMsg::UpdateBus { id, bitrate } => ok_or_error(
            s.session
                .update_bus(&id, bitrate)
                .map(|()| status_of(&s.session)),
        ),
        ClientMsg::RemoveBus { id } => {
            ok_or_error(s.session.remove_bus(&id).map(|()| status_of(&s.session)))
        }
        ClientMsg::AddNode { node } => {
            ok_or_error(s.session.add_node(node).map(|()| status_of(&s.session)))
        }
        ClientMsg::UpdateNode { id, node } => ok_or_error(
            s.session
                .update_node(&id, node)
                .map(|()| status_of(&s.session)),
        ),
        ClientMsg::RemoveNode { id } => {
            ok_or_error(s.session.remove_node(&id).map(|()| status_of(&s.session)))
        }
        ClientMsg::DisableNode { id } => ok_or_error(
            s.session
                .set_node_enabled(&id, false)
                .map(|()| status_of(&s.session)),
        ),
        ClientMsg::EnableNode { id } => ok_or_error(
            s.session
                .set_node_enabled(&id, true)
                .map(|()| status_of(&s.session)),
        ),
        ClientMsg::SaveProject { path } => {
            let target = path.as_deref().map(Path::new);
            match s.session.save(target) {
                Ok(()) => status_of(&s.session),
                Err(e) => ServerMsg::Error {
                    message: e.to_string(),
                },
            }
        }
        ClientMsg::AddMessage { message } => ok_or_error(
            s.session
                .add_message(message)
                .map(|()| status_of(&s.session)),
        ),
        ClientMsg::UpdateMessage { index, message } => ok_or_error(
            s.session
                .update_message(index, message)
                .map(|()| status_of(&s.session)),
        ),
        ClientMsg::RemoveMessage { index } => ok_or_error(
            s.session
                .remove_message(index)
                .map(|()| status_of(&s.session)),
        ),
        ClientMsg::AddFault { fault } => {
            ok_or_error(s.session.add_fault(fault).map(|()| status_of(&s.session)))
        }
        ClientMsg::UpdateFault { index, fault } => ok_or_error(
            s.session
                .update_fault(index, fault)
                .map(|()| status_of(&s.session)),
        ),
        ClientMsg::RemoveFault { index } => ok_or_error(
            s.session
                .remove_fault(index)
                .map(|()| status_of(&s.session)),
        ),
        ClientMsg::GetStatus => status_of(&s.session),
        ClientMsg::GetProject => match s.session.project() {
            Some(p) => ServerMsg::Project { project: p.clone() },
            None => ServerMsg::Error {
                message: "no project loaded".into(),
            },
        },
        ClientMsg::StartRenodeRun { path, runSecs } => {
            // start_renode_job re-locks the state (id reservation, result
            // store) — release the dispatch guard first; std Mutex is not
            // reentrant and this deadlocked the server in testing.
            drop(s);
            start_renode_job(state, path, runSecs)
        }
        ClientMsg::GetRenodeJob { jobId } => match s.jobs.get(jobId) {
            Ok(rec) => ServerMsg::RenodeJob { job: job_info(rec) },
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::CancelRenodeJob { jobId } => match s.jobs.cancel(jobId) {
            Ok(()) => match s.jobs.get(jobId) {
                Ok(rec) => ServerMsg::RenodeJob { job: job_info(rec) },
                Err(e) => ServerMsg::Error {
                    message: e.to_string(),
                },
            },
            Err(e) => ServerMsg::Error {
                message: e.to_string(),
            },
        },
        ClientMsg::ListRenodeJobs => ServerMsg::RenodeJobList {
            jobs: s.jobs.list().iter().map(|r| job_info(r)).collect(),
        },
        ClientMsg::GetRenodeLog { jobId, lastN } => {
            match s.jobs.log_tail(jobId, lastN.unwrap_or(50)) {
                Ok((total, uart)) => ServerMsg::RenodeLog {
                    jobId,
                    total,
                    lines: uart
                        .into_iter()
                        .map(|u| super::proto::UartLine {
                            machine: u.machine,
                            message: u.message,
                        })
                        .collect(),
                },
                Err(e) => ServerMsg::Error {
                    message: e.to_string(),
                },
            }
        }
        ClientMsg::ImportRenodeTrace { jobId } => import_renode_trace(&mut s, jobId),
    }
}

fn ok_or_error(r: Result<ServerMsg, super::session::SessionError>) -> ServerMsg {
    r.unwrap_or_else(|e| ServerMsg::Error {
        message: e.to_string(),
    })
}

/// Validate a renode project file and build its run spec without touching
/// the session. File IO and validation happen outside the server lock;
/// only id reservation takes the lock, and the emulator runs lock-free on
/// a background thread that re-locks solely to store the outcome.
fn start_renode_job(
    state: &Arc<Mutex<ServerState>>,
    path: String,
    run_secs: Option<u64>,
) -> ServerMsg {
    let project_path = Path::new(&path);
    let spec = match load_renode_project(project_path, run_secs.unwrap_or(30)) {
        Ok(spec) => spec,
        Err(message) => return ServerMsg::Error { message },
    };
    // Fail fast on a missing emulator/runtime before reserving a job id.
    let backend = match RenodeBackend::discover() {
        Ok(backend) => backend,
        Err(e) => {
            return ServerMsg::Error {
                message: e.to_string(),
            }
        }
    };
    let (job_id, cancel) = {
        let mut st = state.lock().unwrap();
        let id = match st
            .jobs
            .try_start(project_path.display().to_string(), spec.run_secs)
        {
            Ok(id) => id,
            Err(e) => {
                return ServerMsg::Error {
                    message: e.to_string(),
                }
            }
        };
        // try_start just inserted it; the lookup cannot fail.
        let flag = Arc::clone(&st.jobs.get(id).expect("job just started").cancel);
        (id, flag)
    };
    let shared = Arc::clone(state);
    std::thread::spawn(move || {
        let result = backend.run_cancelable(&spec, &cancel);
        shared.lock().unwrap().jobs.finish(job_id, result);
    });
    ServerMsg::RenodeJobStarted { jobId: job_id }
}

/// Load + validate a renode project file into a run spec. Mirrors the
/// headless CLI contract (all-renode nodes, first bus, project-relative
/// firmware, per-node pre-checks) so GUI and CLI runs agree.
fn load_renode_project(
    path: &Path,
    run_secs: u64,
) -> Result<crate::backends::api::RenodeRunSpec, String> {
    let proj = Project::load_from_file(path)
        .map_err(|e| format!("invalid project {}: {e}", path.display()))?;
    let base = path.parent().unwrap_or(Path::new("."));
    let rep = validate_project(&proj, base);
    if !rep.is_ok() {
        let reasons = rep
            .errors
            .iter()
            .map(|e| format!("[{}] {}", e.path, e.message))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(format!("invalid project {}: {reasons}", path.display()));
    }
    spec_from_project(&proj, path, run_secs).map_err(|e| e.to_string())
}

/// Replay a finished job's observed TX frames through the session engine.
/// Fails when the job is unknown/still running/failed, or when the session
/// holds no project containing the observed senders.
fn import_renode_trace(s: &mut ServerState, job_id: u64) -> ServerMsg {
    let rec = match s.jobs.get(job_id) {
        Ok(rec) => rec,
        Err(e) => {
            return ServerMsg::Error {
                message: e.to_string(),
            }
        }
    };
    match rec.state {
        JobState::Running => {
            return ServerMsg::Error {
                message: format!(
                    "Renode job {job_id} is still running — poll GetRenodeJob until it is done, then import"
                ),
            }
        }
        JobState::Failed => {
            return ServerMsg::Error {
                message: format!(
                    "Renode job {job_id} failed: {}",
                    rec.error.as_deref().unwrap_or("unknown error")
                ),
            }
        }
        JobState::Cancelled => {
            return ServerMsg::Error {
                message: format!("Renode job {job_id} was cancelled — no trace to import"),
            }
        }
        JobState::Done => {}
    }
    let frames: Vec<(String, crate::can::frame::CanFrame)> = match &rec.outcome {
        Some(outcome) => outcome
            .tx_frames
            .iter()
            .map(|f| (f.node.clone(), f.frame.clone()))
            .collect(),
        None => {
            return ServerMsg::Error {
                message: format!(
                    "Renode job {job_id} finished without observations (internal error)"
                ),
            }
        }
    };
    match s.session.import_frames(&frames) {
        Ok(sum) => ServerMsg::TraceImported {
            transmitted: sum.transmitted,
            received: sum.received,
            nextSeq: s.session.events().len(),
        },
        Err(e) => ServerMsg::Error {
            message: e.to_string(),
        },
    }
}

fn job_info(rec: &JobRecord) -> JobInfo {
    JobInfo {
        jobId: rec.id,
        project: rec.project.clone(),
        runSecs: rec.run_secs,
        state: rec.state.name().into(),
        transmitted: rec.transmitted,
        received: rec.received,
        error: rec.error.clone(),
    }
}

/// Build contender frames for an [`ClientMsg::Arbitrate`] round with the
/// same validation as `Inject` (id range, DLC) — the session resolves
/// senders and refuses mixed-bus rounds.
fn arbitrate_frames(
    session: &mut Session,
    frames: &[super::proto::ArbitrateFrame],
) -> Result<super::session::ArbitrationReport, super::session::SessionError> {
    use crate::can::frame::CanFrame;
    use crate::can::id::CanId;
    let mut contenders = Vec::with_capacity(frames.len());
    for f in frames {
        let can_id = if f.extended {
            CanId::new_extended(f.id)
                .map_err(|e| super::session::SessionError::BadFrame(e.to_string()))?
        } else {
            CanId::new_standard(f.id as u16)
                .map_err(|e| super::session::SessionError::BadFrame(e.to_string()))?
        };
        let frame = CanFrame::new(can_id, &f.data)
            .map_err(|e| super::session::SessionError::BadFrame(e.to_string()))?;
        contenders.push((f.sender.clone(), frame));
    }
    session.arbitrate(&contenders)
}

/// Describe one frame's bit-level layout (stateless — needs no session).
/// Same id/DLC validation as `Inject`; remote frames use `dlc` as the
/// requested length and ignore `data`.
fn inspect_frame(
    id: u32,
    extended: bool,
    data: &[u8],
    remote: bool,
    dlc: u8,
) -> Result<ServerMsg, super::session::SessionError> {
    use crate::can::bits::describe_frame;
    use crate::can::frame::CanFrame;
    use crate::can::id::CanId;
    let bad = |e: String| super::session::SessionError::BadFrame(e);
    let can_id = if extended {
        CanId::new_extended(id).map_err(|e| bad(e.to_string()))?
    } else {
        CanId::new_standard(id as u16).map_err(|e| bad(e.to_string()))?
    };
    let frame = if remote {
        CanFrame::new_remote(can_id, dlc).map_err(|e| bad(e.to_string()))?
    } else {
        CanFrame::new(can_id, data).map_err(|e| bad(e.to_string()))?
    };
    let layout = describe_frame(&frame);
    Ok(ServerMsg::FrameBits {
        idHex: format!("{}", frame.id),
        extended,
        dlc: frame.dlc,
        remote,
        regions: layout
            .regions
            .iter()
            .map(|r| super::proto::BitRegion {
                name: r.name.into(),
                offset: r.offset,
                bits: crate::can::bit::to_bit_string(&r.bits),
            })
            .collect(),
        crcHex: format!("{:#06X}", layout.crc),
        stuffBits: layout.stuff_bits,
        wireBits: layout.wire.len(),
        wire: crate::can::bit::to_bit_string(&layout.wire),
    })
}

/// Decode one payload with a DBC file (stateless — needs no session).
/// The file parses on every request (DBC files are small); every parse
/// or decode failure is an actionable `Error`, never a silent misread.
fn decode_frame(dbc: &str, id: u32, data: &[u8]) -> Result<ServerMsg, String> {
    use crate::dbc::Dbc;
    let db = Dbc::load_from_file(std::path::Path::new(dbc)).map_err(|e| e.to_string())?;
    let signals = db.decode(id, data).map_err(|e| e.to_string())?;
    let msg = db.message(id).map_err(|e| e.to_string())?;
    Ok(ServerMsg::DecodedSignals {
        idHex: format!("{id:#X}"),
        message: msg.name.clone(),
        signals: signals
            .into_iter()
            .map(|(name, value, unit)| super::proto::DecodedSignal { name, value, unit })
            .collect(),
    })
}

fn status_of(s: &Session) -> ServerMsg {
    let (buses, nodes) = match s.project() {
        Some(p) => (
            p.buses.iter().map(|b| b.id.clone()).collect(),
            p.nodes.iter().map(|n| n.id.clone()).collect(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    ServerMsg::Status {
        state: state_name(s.state()),
        projectPath: s.project_path().map(|p| p.display().to_string()),
        buses,
        nodes,
        disabled: s.disabled_nodes(),
        nextSeq: s.events().len(),
        dirty: s.dirty(),
    }
}
