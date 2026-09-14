//! Protocol-correctness integration tests (PROMPT.md Phase 2).
//!
//! These exercise the public API end to end: wire-exact delivery, error
//! detection and confinement through the bus, deterministic wire faults,
//! and exact engine timing — no white-box access to internals.

use cansimcan::can::{
    decode_frame, encode_frame, wire_duration_ns, BusEventKind, CanBus, CanBusConfig, CanErrorKind,
    CanFrame, CanId, ErrorState, WireFault,
};
use cansimcan::simulation::engine::Engine;

fn bus_with(nodes: &[&str]) -> CanBus {
    let mut bus = CanBus::new(CanBusConfig::new("test_bus", 500_000).unwrap());
    for n in nodes {
        bus.register_node(*n, 0).unwrap();
    }
    bus
}

fn frame_123() -> CanFrame {
    CanFrame::new(
        CanId::new_standard(0x123).unwrap(),
        &[0x01, 0x02, 0x03, 0x04],
    )
    .unwrap()
}

/// What the transmitter drives is exactly what the codec specifies, and it
/// decodes back to the identical logical frame.
#[test]
fn transmitted_wire_matches_codec() {
    let frame = frame_123();
    let enc = encode_frame(&frame);
    let (decoded, _) = decode_frame(&enc.wire).unwrap();
    assert_eq!(decoded, frame);
    // Sanity: a 4-byte 500 kbit/s frame is ~100 bits → ~200 µs on the wire.
    let ns = wire_duration_ns(&frame, 500_000);
    assert!(ns > 150_000 && ns < 300_000, "unexpected duration {ns} ns");
}

/// Delivery is acknowledged, counters stay clean, and status is observable.
#[test]
fn clean_delivery_keeps_zero_counters() {
    let mut bus = bus_with(&["a", "b"]);
    let out = bus.transmit("a", frame_123(), 0).unwrap();
    assert!(out.acked);
    assert_eq!(out.error, None);
    for node in ["a", "b"] {
        let status = bus.controller_status(node).unwrap();
        assert_eq!(status.tec, 0);
        assert_eq!(status.rec, 0);
        assert_eq!(status.state, ErrorState::Active);
    }
}

/// A flipped wire bit is detected by every receiver as the same kind, the
/// sender counts a transmission error, and nothing is delivered.
#[test]
fn wire_fault_confines_deterministically() {
    let mut bus = bus_with(&["a", "b", "c"]);
    let out = bus
        .transmit_with_fault("a", frame_123(), WireFault::FlipBit(20), 0)
        .unwrap();
    let kind = out.error.expect("fault must be detected");
    assert!(out.receivers.is_empty());
    assert_eq!(bus.controller_status("a").unwrap().tec, 8);
    for node in ["b", "c"] {
        assert_eq!(bus.controller_status(node).unwrap().rec, 1);
    }
    // Both receivers reported the same detection kind.
    let mut kinds: Vec<CanErrorKind> = bus
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            BusEventKind::CanError { node, kind } if node == "b" || node == "c" => Some(*kind),
            _ => None,
        })
        .collect();
    kinds.sort_by_key(|k| *k as u8);
    assert_eq!(kinds, [kind, kind]);
}

/// A lone node parks at error-passive on missing ACKs and never ACKs
/// itself into bus-off (passive-ACK exception, end to end).
#[test]
fn lone_node_parks_at_passive() {
    let mut bus = bus_with(&["only"]);
    for i in 0..64u64 {
        let out = bus.transmit("only", frame_123(), i).unwrap();
        assert!(!out.acked);
    }
    let status = bus.controller_status("only").unwrap();
    assert_eq!(status.tec, 128);
    assert_eq!(status.state, ErrorState::Passive);
}

/// Corrupted frames drive a node all the way to bus-off with visible
/// transitions, and 128 idle sequences recover it.
#[test]
fn bus_off_and_recovery_are_observable() {
    let mut bus = bus_with(&["a", "b"]);
    for i in 0..32u64 {
        bus.transmit_with_fault("a", frame_123(), WireFault::CorruptCrc, i)
            .unwrap();
    }
    assert_eq!(
        bus.controller_status("a").unwrap().state,
        ErrorState::BusOff
    );
    assert!(bus.events().iter().any(|e| matches!(
        &e.kind,
        BusEventKind::ErrorStateChanged {
            state: ErrorState::BusOff,
            ..
        }
    )));
    for t in 32..160u64 {
        bus.note_idle_11(t).unwrap();
    }
    let status = bus.controller_status("a").unwrap();
    assert_eq!(status.state, ErrorState::Active);
    assert_eq!(status.tec, 0);
}

/// The engine steps its clock by the exact stuffed-frame duration and
/// mirrors error events into its log.
#[test]
fn engine_steps_exact_wire_time_and_mirrors_errors() {
    let mut engine = Engine::new();
    engine
        .add_bus(CanBusConfig::new("vehicle_bus", 500_000).unwrap())
        .unwrap();
    engine.register_node("vehicle_bus", "a").unwrap();
    engine.register_node("vehicle_bus", "b").unwrap();
    engine.start();

    let t0 = engine.now();
    engine.transmit("vehicle_bus", "a", frame_123()).unwrap();
    assert_eq!(engine.now() - t0, wire_duration_ns(&frame_123(), 500_000));

    // Solo bus → ACK error mirrored into the engine event log.
    let mut solo = Engine::new();
    solo.add_bus(CanBusConfig::new("solo", 500_000).unwrap())
        .unwrap();
    solo.register_node("solo", "only").unwrap();
    solo.start();
    solo.transmit("solo", "only", frame_123()).unwrap();
    assert!(solo.events().iter().any(|e| matches!(
        &e.kind,
        cansimcan::simulation::event::SimEventKind::BusTraffic(ref be)
        if matches!(be.kind, BusEventKind::CanError { kind: CanErrorKind::Ack, .. })
    )));
}
