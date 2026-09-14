//! First-implementation-task acceptance tests (PROMPT.md §74).
//!
//! These must pass before any GUI work begins:
//!  1. three nodes contend → lowest ID wins, others lose arbitration
//!  2. exact frame delivery (ID 0x123, DLC 8, 01..08)
//!  3. broadcast: every eligible node observes the winning frame

use cansimcan::can::bus::{BusEventKind, CanBus, CanBusConfig};
use cansimcan::can::frame::CanFrame;
use cansimcan::can::id::CanId;

fn test_bus(nodes: &[&str]) -> CanBus {
    let mut bus = CanBus::new(CanBusConfig::new("test_bus", 500_000).unwrap());
    for n in nodes {
        bus.register_node(*n, 0).unwrap();
    }
    bus
}

fn std_frame(id: u16, data: &[u8]) -> CanFrame {
    CanFrame::new(CanId::new_standard(id).unwrap(), data).unwrap()
}

/// §74 test 1: A=0x300, B=0x100, C=0x200 contend → B wins, A and C lose.
#[test]
fn lowest_id_wins_arbitration() {
    let mut bus = test_bus(&["node_a", "node_b", "node_c"]);
    let outcome = bus
        .transmit_simultaneous(
            vec![
                ("node_a".to_string(), std_frame(0x300, &[0xAA])),
                ("node_b".to_string(), std_frame(0x100, &[0xBB])),
                ("node_c".to_string(), std_frame(0x200, &[0xCC])),
            ],
            1_000,
        )
        .unwrap();

    assert_eq!(outcome.winner_node, "node_b");
    assert_eq!(outcome.winner_frame.id, CanId::new_standard(0x100).unwrap());
    assert_eq!(
        outcome.loser_nodes,
        vec!["node_c".to_string(), "node_a".to_string()]
    );

    // Arbitration-loss events name the winner for both losers.
    let losses: Vec<_> = bus
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            BusEventKind::ArbitrationLost { node, winner } => Some((node.clone(), winner.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(losses.len(), 2);
    assert!(losses.contains(&("node_a".to_string(), "node_b".to_string())));
    assert!(losses.contains(&("node_c".to_string(), "node_b".to_string())));
}

/// §74 test 2: exact frame delivery A → B.
#[test]
fn exact_frame_delivery() {
    let mut bus = test_bus(&["node_a", "node_b"]);
    let payload = [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08];
    let frame = std_frame(0x123, &payload);

    let outcome = bus.transmit("node_a", frame.clone(), 500).unwrap();
    assert_eq!(outcome.sender, "node_a");
    assert_eq!(outcome.frame.id, CanId::new_standard(0x123).unwrap());
    assert_eq!(outcome.frame.dlc, 8);
    assert_eq!(outcome.frame.data, payload);
    assert_eq!(outcome.receivers, vec!["node_b".to_string()]);

    // The receiver's FrameReceived event carries the identical frame.
    let received: Vec<_> = bus
        .events()
        .iter()
        .filter_map(|e| match &e.kind {
            BusEventKind::FrameReceived { receiver, frame } if receiver == "node_b" => {
                Some(frame.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(received, vec![frame]);
}

/// §74 test 3: broadcast — every eligible node observes the winning frame.
#[test]
fn all_eligible_nodes_observe_winner() {
    let mut bus = test_bus(&["node_a", "node_b", "node_c"]);
    let frame = std_frame(0x123, &[1, 2, 3, 4]);
    let outcome = bus.transmit("node_a", frame.clone(), 0).unwrap();

    assert_eq!(
        outcome.receivers,
        vec!["node_b".to_string(), "node_c".to_string()]
    );

    let mut seen_by_b = false;
    let mut seen_by_c = false;
    for e in bus.events() {
        if let BusEventKind::FrameReceived { receiver, frame: f } = &e.kind {
            assert_eq!(*f, frame);
            match receiver.as_str() {
                "node_b" => seen_by_b = true,
                "node_c" => seen_by_c = true,
                other => panic!("unexpected receiver {other}"),
            }
        }
    }
    assert!(
        seen_by_b && seen_by_c,
        "both B and C must observe the frame"
    );

    // Sender never receives its own frame back.
    assert!(!bus.events().iter().any(|e| matches!(
        &e.kind,
        BusEventKind::FrameReceived { receiver, .. } if receiver == "node_a"
    )));
}
