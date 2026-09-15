//! Local WebSocket JSON protocol (PROMPT.md §4 "Communication").
//!
//! The contract is deliberately pollable: clients drive the session with
//! [`ClientMsg`] and read the ordered event log with `GetEvents { sinceSeq }`
//! (sequence = index into the session log). Push broadcast comes later;
//! polling keeps v1 correct and testable without async machinery.
//!
//! The TypeScript mirror lives in `frontend/src/api.ts` — keep the two in
//! sync by hand until a generator earns its keep (field names are
//! camelCase on the wire on both sides).

use crate::can::bus::WireFault;
use crate::can::errors::CanErrorKind;
use crate::project::{FaultDecl, MessageDecl, NodeDecl, Project};
use crate::simulation::engine::EngineState;
use crate::simulation::event::{SimEvent, SimEventKind};
use serde::{Deserialize, Serialize};

/// Client → server. Every variant gets exactly one [`ServerMsg`] reply
/// (plus `Error` on failure; the connection stays open).
///
/// NOTE: field names are camelCase on purpose — this is the wire contract
/// shared with `frontend/src/api.ts`, so `non_snake_case` is allowed here
/// and only here.
#[allow(non_snake_case)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMsg {
    /// Load (and validate) a project file; rebuilds the session engine.
    Load {
        path: String,
    },
    /// Run scripted traffic to completion (deterministic).
    Start,
    Pause,
    Resume,
    Stop,
    Reset,
    /// Advance simulated time without traffic.
    Step {
        deltaNs: u64,
    },
    /// Send one ad-hoc frame now.
    Inject {
        sender: String,
        id: u32,
        #[serde(default)]
        extended: bool,
        #[serde(default)]
        data: Vec<u8>,
    },
    /// Drive one deterministically faulted frame now (Phase 9, single-shot).
    /// `fault` is the [`WireFault`] wire shape: `{"FlipBit": offset}`,
    /// `"CorruptCrc"`, or `"DropFrame"`. Out-of-range offsets are an
    /// `Error` reply — never silently clamped.
    InjectFault {
        sender: String,
        id: u32,
        #[serde(default)]
        extended: bool,
        #[serde(default)]
        data: Vec<u8>,
        fault: WireFault,
    },
    /// Ordered events with `seq >= sinceSeq`, plus the new cursor.
    GetEvents {
        #[serde(default)]
        sinceSeq: usize,
    },
    GetStatus,
    GetProject,
    Ping,
    /// Blank unsaved canvas (replaces any loaded project).
    NewProject,
    /// Add a `type: can` bus. Rebuilds the engine; event log restarts.
    AddBus {
        id: String,
        bitrate: u32,
    },
    /// Change a bus bitrate.
    UpdateBus {
        id: String,
        bitrate: u32,
    },
    /// Remove a bus. Refused while nodes attach to it.
    RemoveBus {
        id: String,
    },
    /// Add a node (full declaration incl. `can.bus` attachment).
    /// `backend` must be `virtual` — the live session does not supervise
    /// firmware runs.
    AddNode {
        node: NodeDecl,
    },
    /// Replace a node's declaration (`node.id` must equal `id`; no renames).
    UpdateNode {
        id: String,
        node: NodeDecl,
    },
    /// Remove a node. Refused while scripted `messages:` reference it.
    RemoveNode {
        id: String,
    },
    /// Validate strictly and write the project YAML. `path` overrides the
    /// loaded path (save-as); without either there is nowhere to write.
    SaveProject {
        #[serde(default)]
        path: Option<String>,
    },
    /// Append one scripted frame to `messages:` (sender must be a loaded
    /// node; id range and DLC enforced like `validate_project`).
    AddMessage {
        message: MessageDecl,
    },
    /// Replace the scripted frame at `index` (shifts after deletes — see
    /// bugs.md; refresh the list after every op).
    UpdateMessage {
        index: usize,
        message: MessageDecl,
    },
    /// Delete the scripted frame at `index`.
    RemoveMessage {
        index: usize,
    },
    /// Append one fault policy to `faults:` (node/id narrow eligibility;
    /// probability in [0.0, 1.0]; seed fixes the draw stream).
    AddFault {
        fault: FaultDecl,
    },
    /// Replace the fault policy at `index` (shifts after deletes — see
    /// bugs.md; refresh the list after every op).
    UpdateFault {
        index: usize,
        fault: FaultDecl,
    },
    /// Delete the fault policy at `index`.
    RemoveFault {
        index: usize,
    },
    /// Start a supervised Renode firmware run in the background (job
    /// table). `path` is a project file with all nodes on
    /// `backend: renode`; `runSecs` defaults to 30 (firmware loops
    /// forever, so the budget ending is the normal end). Replies
    /// `RenodeJobStarted` immediately — polling `GetRenodeJob` tracks it.
    StartRenodeRun {
        path: String,
        #[serde(default)]
        runSecs: Option<u64>,
    },
    /// Poll one background firmware run.
    GetRenodeJob {
        jobId: u64,
    },
    /// Request cancellation of a running firmware run (async: the record
    /// turns `cancelled` on the emulator thread's next tick). Replies the
    /// current `RenodeJob`; already-finished jobs are an `Error`.
    CancelRenodeJob {
        jobId: u64,
    },
    /// Last UART lines of a job (running jobs report what they have so
    /// far). `lastN` defaults to 50, capped server-side at 200 — full logs
    /// stay CLI-only.
    GetRenodeLog {
        jobId: u64,
        #[serde(default)]
        lastN: Option<usize>,
    },
    /// List background firmware runs, oldest first.
    ListRenodeJobs,
    /// Replay a finished job's observed TX frames through the session
    /// engine (batch-inject at current engine time). Replies
    /// `TraceImported`; the analyzer then shows firmware traffic.
    ImportRenodeTrace {
        jobId: u64,
    },
}

/// Server → client (camelCase wire contract — see [`ClientMsg`]).
#[allow(non_snake_case)]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ServerMsg {
    Status {
        state: String,
        projectPath: Option<String>,
        buses: Vec<String>,
        nodes: Vec<String>,
        nextSeq: usize,
        dirty: bool,
    },
    Events {
        events: Vec<SeqEvent>,
        nextSeq: usize,
    },
    Project {
        project: Project,
    },
    RunSummary {
        transmitted: u64,
        received: u64,
        nextSeq: usize,
    },
    Stepped {
        nowNs: u64,
        nextSeq: usize,
    },
    Injected {
        receivers: usize,
        nextSeq: usize,
    },
    FaultInjected {
        error: Option<CanErrorKind>,
        receivers: usize,
        nextSeq: usize,
    },
    RenodeJobStarted {
        jobId: u64,
    },
    RenodeJob {
        job: JobInfo,
    },
    RenodeJobList {
        jobs: Vec<JobInfo>,
    },
    TraceImported {
        transmitted: u64,
        received: u64,
        nextSeq: usize,
    },
    RenodeLog {
        jobId: u64,
        total: u64,
        lines: Vec<UartLine>,
    },
    Pong,
    Error {
        message: String,
    },
}

/// One background firmware run, as reported over the wire. Counts come
/// from the finished run's parsed UART observations; the full log stays
/// server-side (it can be megabytes) — the CLI remains the place for it.
#[allow(non_snake_case)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInfo {
    pub jobId: u64,
    pub project: String,
    pub runSecs: u64,
    pub state: String,
    pub transmitted: u64,
    pub received: u64,
    pub error: Option<String>,
}

/// One firmware UART line over the wire (see `GetRenodeLog`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UartLine {
    pub machine: String,
    pub message: String,
}

/// One log entry with its sequence number (index in the session log).
#[allow(non_snake_case)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeqEvent {
    pub seq: usize,
    pub timeNs: u64,
    pub kind: SimEventKind,
}

pub fn state_name(state: EngineState) -> String {
    match state {
        EngineState::Idle => "idle",
        EngineState::Running => "running",
        EngineState::Paused => "paused",
        EngineState::Stopped => "stopped",
    }
    .into()
}

/// Serialize one [`SimEvent`] with its sequence number.
pub fn seq_event(seq: usize, event: &SimEvent) -> SeqEvent {
    SeqEvent {
        seq,
        timeNs: event.time_ns,
        kind: event.kind.clone(),
    }
}

/// Parse one inbound text frame. A wrong-shape message is a client bug —
/// report it as a string, never panic the connection.
pub fn parse_client_msg(text: &str) -> Result<ClientMsg, String> {
    serde_json::from_str(text).map_err(|e| format!("bad request (expected {{\"type\": ...}}): {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_names_are_camel_case_and_tagged() {
        let msg = ClientMsg::Inject {
            sender: "a".into(),
            id: 0x123,
            extended: false,
            data: vec![1],
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["type"], "Inject");
        assert_eq!(json["sender"], "a");
        let back: ClientMsg = serde_json::from_value(json).unwrap();
        assert!(matches!(back, ClientMsg::Inject { .. }));
    }

    #[test]
    fn server_messages_round_trip() {
        let msg = ServerMsg::RunSummary {
            transmitted: 1,
            received: 2,
            nextSeq: 9,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"RunSummary\""));
        assert!(json.contains("\"nextSeq\":9"));
    }

    #[test]
    fn edit_messages_round_trip() {
        for msg in [
            ClientMsg::NewProject,
            ClientMsg::AddBus {
                id: "b".into(),
                bitrate: 500_000,
            },
            ClientMsg::UpdateBus {
                id: "b".into(),
                bitrate: 250_000,
            },
            ClientMsg::RemoveBus { id: "b".into() },
            ClientMsg::RemoveNode { id: "n".into() },
            ClientMsg::SaveProject { path: None },
            ClientMsg::SaveProject {
                path: Some("p.canlab".into()),
            },
        ] {
            let json = serde_json::to_value(&msg).unwrap();
            let back: ClientMsg = serde_json::from_value(json).unwrap();
            assert_eq!(
                serde_json::to_value(&back).unwrap()["type"],
                serde_json::to_value(&msg).unwrap()["type"]
            );
        }
        // NodeDecl serializes with its serde names; the GUI builds it raw.
        let add: ClientMsg = serde_json::from_value(serde_json::json!({
            "type": "AddNode",
            "node": {"id": "n", "device": "stm32f103", "backend": "virtual",
                      "can": {"bus": "b"}}
        }))
        .unwrap();
        assert!(matches!(add, ClientMsg::AddNode { .. }));
    }

    #[test]
    fn message_ops_round_trip() {
        let add = ClientMsg::AddMessage {
            message: MessageDecl {
                sender: "n".into(),
                id: 0x123,
                data: vec![1, 2],
                extended: false,
            },
        };
        let json = serde_json::to_value(&add).unwrap();
        assert_eq!(json["type"], "AddMessage");
        assert_eq!(json["message"]["sender"], "n");
        let back: ClientMsg = serde_json::from_value(json).unwrap();
        assert!(matches!(back, ClientMsg::AddMessage { .. }));
        for raw in [
            r#"{"type":"UpdateMessage","index":2,"message":{"sender":"n","id":1,"data":[]}}"#,
            r#"{"type":"RemoveMessage","index":0}"#,
        ] {
            assert!(parse_client_msg(raw).is_ok());
        }
    }

    #[test]
    fn fault_ops_round_trip() {
        use crate::can::bus::WireFault;
        let add = ClientMsg::AddFault {
            fault: FaultDecl {
                fault: WireFault::CorruptCrc,
                node: Some("n".into()),
                id: None,
                extended: false,
                probability: 0.25,
                seed: 11,
            },
        };
        let json = serde_json::to_value(&add).unwrap();
        assert_eq!(json["type"], "AddFault");
        assert_eq!(json["fault"]["node"], "n");
        assert_eq!(json["fault"]["probability"], 0.25);
        let back: ClientMsg = serde_json::from_value(json).unwrap();
        assert!(matches!(back, ClientMsg::AddFault { .. }));
        for raw in [
            r#"{"type":"AddFault","fault":{"fault":"DropFrame","probability":1.0,"seed":5}}"#,
            r#"{"type":"AddFault","fault":{"fault":{"FlipBit":30},"probability":0.5,"seed":5}}"#,
            r#"{"type":"UpdateFault","index":1,"fault":{"fault":"DropFrame","probability":0.0,"seed":0}}"#,
            r#"{"type":"RemoveFault","index":0}"#,
        ] {
            assert!(parse_client_msg(raw).is_ok(), "{raw}");
        }
    }

    #[test]
    fn garbage_is_a_string_not_a_panic() {
        assert!(parse_client_msg("hello").is_err());
        assert!(parse_client_msg("{\"type\":\"Nope\"}").is_err());
        assert!(parse_client_msg("{\"type\":\"Step\",\"deltaNs\":5}").is_ok());
    }

    #[test]
    fn renode_job_messages_round_trip() {
        for raw in [
            r#"{"type":"StartRenodeRun","path":"renode.canlab"}"#,
            r#"{"type":"StartRenodeRun","path":"renode.canlab","runSecs":45}"#,
            r#"{"type":"GetRenodeJob","jobId":2}"#,
            r#"{"type":"CancelRenodeJob","jobId":2}"#,
            r#"{"type":"GetRenodeLog","jobId":2}"#,
            r#"{"type":"GetRenodeLog","jobId":2,"lastN":20}"#,
            r#"{"type":"ListRenodeJobs"}"#,
            r#"{"type":"ImportRenodeTrace","jobId":2}"#,
        ] {
            assert!(parse_client_msg(raw).is_ok(), "{raw}");
        }
        // runSecs is optional on the wire.
        let bare: ClientMsg =
            serde_json::from_str(r#"{"type":"StartRenodeRun","path":"p.canlab"}"#).unwrap();
        assert!(matches!(
            bare,
            ClientMsg::StartRenodeRun { runSecs: None, .. }
        ));
        let info = JobInfo {
            jobId: 1,
            project: "p.canlab".into(),
            runSecs: 30,
            state: "done".into(),
            transmitted: 5,
            received: 5,
            error: None,
        };
        let json = serde_json::to_value(&ServerMsg::RenodeJobList { jobs: vec![info] }).unwrap();
        assert_eq!(json["jobs"][0]["jobId"], 1);
        assert_eq!(json["jobs"][0]["runSecs"], 30);
    }
}
