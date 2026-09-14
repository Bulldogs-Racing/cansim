# Getting started

CanLab is CAN-first: simulate real CAN traffic between virtual nodes today,
real MCU firmware (via Renode) from Phase 4 on.

## Prerequisites

- Rust 1.85+ (`cargo --version`)
- Node.js 20+ (only for the Phase 5 frontend shell)

## Run your first headless simulation

```bash
cargo run -q --bin canlab -- simulate examples/two_nodes.canlab.yaml
```

Expected output (timestamps are simulated, deterministic):

```text
Starting CanLab...

Bus:
  vehicle_bus
  bitrate: 500000

Nodes:
  engine_ecu
  dashboard

Simulation started.

[0.000 ms] engine_ecu TX 0x123 [01 02 03 04]
[0.000 ms] dashboard RX 0x123 [01 02 03 04]
...

Simulation finished: 2 frame(s) transmitted, 2 reception(s).
```

## Scaffold and validate a project

```bash
cargo run -q --bin canlab -- new my-project
cargo run -q --bin canlab -- validate my-project/project.canlab
cargo run -q --bin canlab -- doctor
```

## Run the tests

```bash
cargo test
```

The §74 acceptance tests live in `tests/can_core.rs` and must stay green
before any GUI work begins.

## Project format

See `docs/projects.md`. TL;DR: versioned YAML (`version: 1`), relative
firmware paths, `fd: false` until CAN FD lands. Unknown devices/backends
fail validation fast instead of mis-simulating.
