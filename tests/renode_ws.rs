//! Live Renode-over-WebSocket test: background firmware run end to end.
//!
//! Ignored by default — needs the repo-local ARM toolchain output, a
//! Renode install *with* its .NET runtime, and ~40 s of wall clock:
//!
//! ```bash
//! ./firmware/tests/stm32_can/build.sh
//! export PATH="$PWD/.tools/dotnet:$PATH"
//! cargo test --test renode_ws -- --ignored --nocapture
//! ```
//!
//! Flow: load a virtual session project, `StartRenodeRun` an all-renode
//! project, poll the job to `done`, `ImportRenodeTrace` the observations
//! into the session, and assert the analyzer stream carries the firmware
//! frames. Fails loudly (not silently skipped) when only *some*
//! prerequisites are present; skips only when the fixture ELFs were never
//! built.

use cansimcan::server::serve::{serve_ephemeral, ServerState};
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

#[test]
#[ignore]
fn live_renode_job_imports_firmware_traffic() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for elf in [
        "firmware/tests/stm32_can/build/can_tx.elf",
        "firmware/tests/stm32_can/build/can_rx.elf",
    ] {
        if !root.join(elf).is_file() {
            eprintln!(
                "SKIP: fixture ELFs not built; run ./firmware/tests/stm32_can/build.sh first"
            );
            return;
        }
    }

    let port = serve_ephemeral(Arc::new(Mutex::new(ServerState::new(1))));
    let (mut ws, _) = tungstenite::connect(format!("ws://127.0.0.1:{port}")).unwrap();

    // Virtual session whose nodes match the firmware senders (import target).
    let req =
        serde_json::json!({"type": "Load", "path": "examples/two_nodes.canlab.yaml"}).to_string();
    assert_eq!(rpc(&mut ws, &req)["type"], "Status");

    // Start the firmware run in the background (prompt reply, no blocking).
    let req = serde_json::json!({"type": "StartRenodeRun", "path": "firmware/tests/stm32_can/two_nodes.canlab.yaml", "runSecs": 30}).to_string();
    let started = rpc(&mut ws, &req);
    assert_eq!(started["type"], "RenodeJobStarted", "{started}");
    let job_id = started["jobId"].as_u64().unwrap();

    // A second concurrent run is refused at the default --max-jobs 1.
    let req = serde_json::json!({"type": "StartRenodeRun", "path": "firmware/tests/stm32_can/two_nodes.canlab.yaml", "runSecs": 5}).to_string();
    let err = rpc(&mut ws, &req);
    assert_eq!(err["type"], "Error", "{err}");
    assert!(err["message"].as_str().unwrap().contains("--max-jobs"));

    // Poll until done (30 s budget + emulator boot margin).
    let req = format!(r#"{{"type":"GetRenodeJob","jobId":{job_id}}}"#);
    let mut state = String::new();
    for _ in 0..60 {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let job = rpc(&mut ws, &req);
        state = job["job"]["state"].as_str().unwrap().to_string();
        if state != "running" {
            break;
        }
    }
    assert_eq!(
        state,
        "done",
        "firmware job did not finish; last poll: {}",
        rpc(&mut ws, &req)
    );

    // Import replays observed TX frames through the session engine.
    let req = format!(r#"{{"type":"ImportRenodeTrace","jobId":{job_id}}}"#);
    let imported = rpc(&mut ws, &req);
    assert_eq!(imported["type"], "TraceImported", "{imported}");
    assert_eq!(imported["transmitted"], 5);
    assert_eq!(imported["received"], 5);

    // Analyzer stream carries the firmware frames (0x123 = 291).
    let events = rpc(&mut ws, r#"{"type":"GetEvents","sinceSeq":0}"#);
    let text = serde_json::to_string(&events).unwrap();
    assert!(
        text.contains("\"Standard\":291"),
        "firmware id must reach the stream"
    );
}
