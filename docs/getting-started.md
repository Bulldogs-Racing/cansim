# Getting started

CanLab is CAN-first: simulate real CAN traffic between virtual nodes today,
real MCU firmware (via Renode) from Phase 4 on.

## Prerequisites

- Rust 1.85+ (`cargo --version`)
- Node.js 20+ (only for the Phase 5 frontend shell)

## Local tools (`.tools/`, no root needed)

Everything external lives in the gitignored `.tools/` directory —
nothing is installed system-wide:

```text
.tools/
├── renode -> <your Renode portable install>   # emulation backend (Phase 4)
└── arm-gcc -> arm-gnu-toolchain-*/            # STM32 firmware builds
```

- **Renode**: unpack a `linux-*-portable` build and link it as
  `.tools/renode` (the portable build embeds the dotnet runtime).
- **ARM toolchain**: unpack an `aarch64`-hosted `arm-none-eabi` tarball
  from ARM's GNU toolchain downloads and link it as `.tools/arm-gcc`
  (14.2.rel1 verified: `arm-none-eabi-gcc -mcpu=cortex-m3` emits ARM ELF32).
- Override the location with `CANLAB_TOOLS=/path/to/tools`.
- Check the setup with `canlab doctor` (run from the repo root).

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
