//! Live Renode backend test: real STM32F103 firmware end to end.
//!
//! Ignored by default — needs the repo-local ARM toolchain output and a
//! Renode install:
//!
//! ```bash
//! ./firmware/tests/stm32_can/build.sh
//! cargo test --test renode_backend -- --ignored --nocapture
//! ```
//!
//! The test fails loudly (not silently skipped) when only *some*
//! prerequisites are present; it skips only when the fixture ELFs were
//! never built.

use cansimcan::backends::api::{McuBackend, NodeFirmware, RenodeRunSpec};
use cansimcan::backends::renode::RenodeBackend;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
#[ignore]
fn live_two_node_can_exchange() {
    let root = repo_root();
    let tx = root.join("firmware/tests/stm32_can/build/can_tx.elf");
    let rx = root.join("firmware/tests/stm32_can/build/can_rx.elf");
    if !tx.is_file() || !rx.is_file() {
        eprintln!("SKIP: fixture ELFs not built; run ./firmware/tests/stm32_can/build.sh first");
        return;
    }
    let backend = RenodeBackend::discover().expect("renode must be installed for the live test");
    let spec = RenodeRunSpec {
        bus_id: "vehicle_bus".into(),
        bitrate: 500_000,
        nodes: vec![
            NodeFirmware {
                node_id: "engine_ecu".into(),
                machine: NodeFirmware::machine_name("engine_ecu"),
                device: "stm32f103".into(),
                elf: tx,
            },
            NodeFirmware {
                node_id: "dashboard".into(),
                machine: NodeFirmware::machine_name("dashboard"),
                device: "stm32f103".into(),
                elf: rx,
            },
        ],
        run_secs: 60,
    };
    let outcome = backend.run(&spec).expect("renode run must succeed");
    assert!(
        !outcome.tx_frames.is_empty(),
        "expected firmware TX frames, uart log:\n{}",
        outcome
            .uart
            .iter()
            .map(|u| format!("[{}] {}", u.machine, u.message))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        !outcome.rx_frames.is_empty(),
        "expected firmware RX frames (delivery proof)"
    );
    // Every reported reception matches a transmission (id + payload).
    let tx_set: std::collections::HashSet<String> = outcome
        .tx_frames
        .iter()
        .map(|f| format!("{}|{}", f.frame.id, f.frame.data_hex()))
        .collect();
    for rx in &outcome.rx_frames {
        let key = format!("{}|{}", rx.frame.id, rx.frame.data_hex());
        assert!(tx_set.contains(&key), "orphan RX without TX: {key}");
    }
    assert_eq!(
        outcome.tx_frames[0].frame.data.as_slice(),
        &[1, 2, 3, 4, 5, 6, 7, 8]
    );
}
