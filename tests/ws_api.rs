//! WebSocket API test: real client ↔ real server over loopback.
//!
//! Spins `serve_ephemeral` (random port, background thread), drives it with
//! a synchronous tungstenite client through Load → Start → GetEvents →
//! Inject → Reset, and asserts the event stream carries the expected frames.
//! No browser needed; the same JSON contract `frontend/src/api.ts` uses.

use cansimcan::server::serve::serve_ephemeral;
use cansimcan::server::serve::ServerState;
use std::sync::{Arc, Mutex};
use tungstenite::Message;

fn rpc<S: std::io::Read + std::io::Write>(
    ws: &mut tungstenite::WebSocket<S>,
    text: &str,
) -> serde_json::Value {
    ws.send(Message::Text(text.into())).unwrap();
    match ws.read().unwrap() {
        Message::Text(t) => serde_json::from_str(&t).unwrap(),
        other => panic!("expected text reply, got {other:?}"),
    }
}

const PROJECT: &str = r#"
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

#[test]
fn ws_load_start_events_inject_reset() {
    let dir = std::env::temp_dir().join(format!("canlab-ws-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let proj = dir.join("p.canlab");
    std::fs::write(&proj, PROJECT).unwrap();

    let port = serve_ephemeral(Arc::new(Mutex::new(ServerState::new(1))));
    let (mut ws, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}")).unwrap();

    // No project yet: status is idle, Start fails with guidance.
    let status = rpc(&mut ws, r#"{"type":"GetStatus"}"#);
    assert_eq!(status["state"], "idle");
    let err = rpc(&mut ws, r#"{"type":"Start"}"#);
    assert_eq!(err["type"], "Error");

    // Load → project echoed in status.
    let req = serde_json::json!({"type": "Load", "path": proj.display().to_string()}).to_string();
    let status = rpc(&mut ws, &req);
    assert_eq!(status["type"], "Status");
    assert_eq!(
        status["nodes"],
        serde_json::json!(["engine_ecu", "dashboard"])
    );

    // Start → summary + events carry the 0x123 frame to dashboard.
    let pre = rpc(&mut ws, r#"{"type":"GetStatus"}"#);
    let pre_seq = pre["nextSeq"].as_u64().unwrap() as usize;
    let summary = rpc(&mut ws, r#"{"type":"Start"}"#);
    assert_eq!(summary["type"], "RunSummary");
    assert_eq!(summary["transmitted"], 1);
    assert_eq!(summary["received"], 1);
    let next_seq = summary["nextSeq"].as_u64().unwrap() as usize;

    let events = rpc(&mut ws, r#"{"type":"GetEvents","sinceSeq":0}"#);
    assert_eq!(events["type"], "Events");
    let rows = events["events"].as_array().unwrap();
    assert!(!rows.is_empty());
    assert!(rows.iter().enumerate().all(|(i, e)| e["seq"] == i as u64));
    let text = serde_json::to_string(&events).unwrap();
    // Wire shape is structural (CanId serializes as {"Standard":291});
    // the analyzer formats it. Payload bytes must reach the stream.
    assert!(
        text.contains("\"Standard\":291"),
        "frame id must reach the analyzer stream"
    );
    assert!(
        text.contains("[1,2,3,4]"),
        "payload must reach the analyzer stream"
    );

    // Wire-nesting contract for the analyzer (frontend analyzerRows):
    // SeqEvent.kind = { BusTraffic: BusEvent }, BusEvent.kind = variant.
    // A flattened shape would silently empty the analyzer table.
    let rows = events["events"].as_array().unwrap();
    let tx = rows.iter().find(|e| {
        e["kind"]["BusTraffic"]["kind"]
            .get("FrameTransmitted")
            .is_some()
    });
    assert_eq!(
        tx.unwrap()["kind"]["BusTraffic"]["kind"]["FrameTransmitted"]["sender"],
        "engine_ecu"
    );
    assert!(
        rows.iter().any(|e| e["kind"]["BusTraffic"]["kind"]
            .get("FrameReceived")
            .is_some()),
        "a FrameReceived delivery must be nested the same way"
    );

    // Fetch-then-adopt contract (the GUI fetches since the PRE-start cursor
    // before adopting Status.nextSeq): the Start-grown frames must sit
    // strictly between the pre-start cursor and the post-start head.
    // Adopting the head first would skip exactly the frames just produced.
    let req = format!(r#"{{"type":"GetEvents","sinceSeq":{pre_seq}}}"#);
    let grown = rpc(&mut ws, &req);
    let grown_rows = grown["events"].as_array().unwrap();
    assert!(!grown_rows.is_empty());
    assert!(
        grown_rows
            .iter()
            .enumerate()
            .all(|(i, e)| e["seq"] == (pre_seq + i) as u64),
        "grown events must continue the pre-start sequence without gaps"
    );
    assert!(
        serde_json::to_string(&grown)
            .unwrap()
            .contains("\"Standard\":291"),
        "the 0x123 frame must be fetchable since the pre-start cursor"
    );

    // Polling with a fresh cursor yields nothing new.
    let req = format!(r#"{{"type":"GetEvents","sinceSeq":{next_seq}}}"#);
    let empty = rpc(&mut ws, &req);
    assert_eq!(empty["events"].as_array().unwrap().len(), 0);

    // Inject → dashboard's frame reaches engine_ecu; visible after cursor.
    let injected = rpc(
        &mut ws,
        r#"{"type":"Inject","sender":"dashboard","id":512,"data":[9]}"#,
    );
    assert_eq!(injected["type"], "Injected");
    assert_eq!(injected["receivers"], 1);
    let req = format!(r#"{{"type":"GetEvents","sinceSeq":{next_seq}}}"#);
    let more = rpc(&mut ws, &req);
    assert!(serde_json::to_string(&more)
        .unwrap()
        .contains("\"Standard\":512"));

    // Bad input is an Error reply, not a dropped connection.
    let err = rpc(
        &mut ws,
        r#"{"type":"Inject","sender":"ghost","id":1,"data":[]}"#,
    );
    assert_eq!(err["type"], "Error");
    let err = rpc(&mut ws, "not json");
    assert_eq!(err["type"], "Error");

    // Reset → idle again, log keeps working.
    let status = rpc(&mut ws, r#"{"type":"Reset"}"#);
    assert_eq!(status["state"], "idle");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ws_edit_save_flow() {
    let dir = std::env::temp_dir().join(format!("canlab-ws-edit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("built.canlab");

    let port = serve_ephemeral(Arc::new(Mutex::new(ServerState::new(1))));
    let (mut ws, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}")).unwrap();

    // Blank canvas: no path, dirty, cannot run yet.
    let status = rpc(&mut ws, r#"{"type":"NewProject"}"#);
    assert_eq!(status["type"], "Status");
    assert_eq!(status["projectPath"], serde_json::Value::Null);
    assert_eq!(status["dirty"], true);
    assert_eq!(status["nextSeq"], 0);
    assert_eq!(rpc(&mut ws, r#"{"type":"Start"}"#)["type"], "Error");

    // Bus + node; duplicate bus is an Error, session untouched.
    let status = rpc(&mut ws, r#"{"type":"AddBus","id":"b","bitrate":500000}"#);
    assert_eq!(status["buses"], serde_json::json!(["b"]));
    assert_eq!(status["nextSeq"], 0); // edit rebuilt the engine
    let err = rpc(&mut ws, r#"{"type":"AddBus","id":"b","bitrate":500000}"#);
    assert_eq!(err["type"], "Error");
    let status = rpc(
        &mut ws,
        r#"{"type":"AddNode","node":{"id":"n","device":"stm32f103","backend":"virtual","can":{"bus":"b"}}}"#,
    );
    assert_eq!(status["nodes"], serde_json::json!(["n"]));
    // Renode nodes are refused with a headless pointer.
    let err = rpc(
        &mut ws,
        r#"{"type":"AddNode","node":{"id":"fw","device":"stm32f103","backend":"renode","can":{"bus":"b"}}}"#,
    );
    assert_eq!(err["type"], "Error");
    assert!(err["message"].as_str().unwrap().contains("canlab simulate"));

    // Rename via UpdateNode is refused; re-attach works via canvas drag.
    let err = rpc(
        &mut ws,
        r#"{"type":"UpdateNode","id":"n","node":{"id":"m","device":"stm32f103","backend":"virtual","can":{"bus":"b"}}}"#,
    );
    assert_eq!(err["type"], "Error");
    let status = rpc(
        &mut ws,
        r#"{"type":"UpdateNode","id":"n","node":{"id":"n","device":"arduino_uno","backend":"virtual","can":{"bus":"b"}}}"#,
    );
    assert_eq!(status["type"], "Status");
    let proj = rpc(&mut ws, r#"{"type":"GetProject"}"#);
    assert_eq!(proj["project"]["nodes"][0]["device"], "arduino_uno");

    // Bus in use cannot go; saving validates and clears dirty.
    let err = rpc(&mut ws, r#"{"type":"RemoveBus","id":"b"}"#);
    assert_eq!(err["type"], "Error");
    let req = serde_json::json!({"type": "SaveProject", "path": target.display().to_string()})
        .to_string();
    let status = rpc(&mut ws, &req);
    assert_eq!(status["type"], "Status");
    assert_eq!(status["dirty"], false);
    assert_eq!(
        status["projectPath"],
        serde_json::json!(target.display().to_string())
    );
    let text = std::fs::read_to_string(&target).unwrap();
    assert!(text.contains("arduino_uno"));

    // Saved file reloads clean through a fresh Load.
    let req = serde_json::json!({"type": "Load", "path": target.display().to_string()}).to_string();
    let status = rpc(&mut ws, &req);
    assert_eq!(status["nodes"], serde_json::json!(["n"]));
    assert_eq!(status["dirty"], false);

    // Scripted messages: add → runs → update → remove; bad frames refused.
    let added = rpc(
        &mut ws,
        r#"{"type":"AddMessage","message":{"sender":"n","id":291,"data":[1,2]}}"#,
    );
    assert_eq!(added["type"], "Status");
    assert_eq!(added["dirty"], true);
    let err = rpc(
        &mut ws,
        r#"{"type":"AddMessage","message":{"sender":"ghost","id":1,"data":[]}}"#,
    );
    assert_eq!(err["type"], "Error");
    let err = rpc(
        &mut ws,
        r#"{"type":"AddMessage","message":{"sender":"n","id":2048,"data":[]}}"#,
    );
    assert_eq!(err["type"], "Error");
    let summary = rpc(&mut ws, r#"{"type":"Start"}"#);
    assert_eq!(summary["transmitted"], 1);
    let updated = rpc(
        &mut ws,
        r#"{"type":"UpdateMessage","index":0,"message":{"sender":"n","id":512,"data":[9]}}"#,
    );
    assert_eq!(updated["type"], "Status");
    let proj = rpc(&mut ws, r#"{"type":"GetProject"}"#);
    assert_eq!(proj["project"]["messages"][0]["id"], 512);
    assert_eq!(
        rpc(
            &mut ws,
            r#"{"type":"UpdateMessage","index":3,"message":{"sender":"n","id":1,"data":[]}}"#
        )["type"],
        "Error"
    );
    let removed = rpc(&mut ws, r#"{"type":"RemoveMessage","index":0}"#);
    assert_eq!(removed["type"], "Status");
    let proj = rpc(&mut ws, r#"{"type":"GetProject"}"#);
    assert_eq!(proj["project"]["messages"].as_array().unwrap().len(), 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ws_renode_jobs_reject_bad_input_without_emulator() {
    let dir = std::env::temp_dir().join(format!("canlab-ws-renode-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let port = serve_ephemeral(Arc::new(Mutex::new(ServerState::new(1))));
    let (mut ws, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}")).unwrap();

    // Empty table lists clean.
    let list = rpc(&mut ws, r#"{"type":"ListRenodeJobs"}"#);
    assert_eq!(list["type"], "RenodeJobList");
    assert_eq!(list["jobs"].as_array().unwrap().len(), 0);

    // Missing project file: Error, no job created.
    let req = serde_json::json!({"type": "StartRenodeRun", "path": dir.join("nope.canlab").display().to_string()}).to_string();
    let err = rpc(&mut ws, &req);
    assert_eq!(err["type"], "Error");
    let list = rpc(&mut ws, r#"{"type":"ListRenodeJobs"}"#);
    assert_eq!(list["jobs"].as_array().unwrap().len(), 0);

    // Virtual project: all-renode contract fails before any spawn.
    let virt = dir.join("virt.canlab");
    std::fs::write(
        &virt,
        "version: 1\nsimulation: {mode: deterministic}\nbuses:\n  - {id: b, type: can, bitrate: 500000, fd: false}\nnodes:\n  - {id: n, device: stm32f103, backend: virtual, can: {bus: b}}\n",
    )
    .unwrap();
    let req = serde_json::json!({"type": "StartRenodeRun", "path": virt.display().to_string()})
        .to_string();
    let err = rpc(&mut ws, &req);
    assert_eq!(err["type"], "Error");
    assert!(err["message"].as_str().unwrap().contains("virtual"));

    // Unknown job ids name themselves on poll and import.
    let err = rpc(&mut ws, r#"{"type":"GetRenodeJob","jobId":99}"#);
    assert_eq!(err["type"], "Error");
    assert!(err["message"].as_str().unwrap().contains("99"));
    let err = rpc(&mut ws, r#"{"type":"ImportRenodeTrace","jobId":99}"#);
    assert_eq!(err["type"], "Error");

    let _ = std::fs::remove_dir_all(&dir);
}
