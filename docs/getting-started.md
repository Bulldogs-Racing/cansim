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
├── dotnet/                                    # .NET 8+ runtime for the Renode launcher
└── arm-gcc -> arm-gnu-toolchain-*/            # STM32 firmware builds
```

- **Renode**: unpack a `linux-*-portable` build and link it as
  `.tools/renode` (the portable build embeds the dotnet runtime).
  A system `renode` package works too — but its launcher still needs a
  .NET 8+ runtime (see next line).
- **dotnet runtime** (only if your `renode` needs it — `renode --version`
  says so): install repo-locally, no root needed:
  ```bash
  curl -sSL https://dot.net/v1/dotnet-install.sh -o /tmp/dotnet-install.sh
  bash /tmp/dotnet-install.sh --channel 8.0 --runtime dotnet --install-dir ./.tools/dotnet --no-path
  export PATH="$PWD/.tools/dotnet:$PATH"
  ```
  `canlab doctor` reports the runtime version; `RenodeBackend::discover`
  fails fast with install instructions when it is missing (instead of a
  cryptic mid-run emulator exit).
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

## Run real STM32F103 firmware (Renode backend)

Build the fixture firmware, then simulate an all-`renode` project:

```bash
./firmware/tests/stm32_can/build.sh
cargo run -q --bin canlab -- simulate <your-renode-project.canlab> --run-secs 30
```

A Renode project looks like the virtual one, but every node uses
`backend: renode`, `device: stm32f103`, and a project-relative `firmware:`
path to a real `.elf` you built yourself (copy/paste is fine — no IDE
integration needed). `canlab simulate` boots one emulated node per project
node, joins their CAN1s on a virtual hub, captures UART output, and replays
observed frames through the deterministic engine. Only `stm32f103` executes;
other devices fail with an explicit error. See `docs/mcu-backends.md` for
run semantics (`--run-secs` is a duration, not a failure timeout).

## Run the visual editor

Terminal 1 — API (from the repo root, so relative project paths resolve):

```bash
cargo run -q --bin canlab -- serve --project examples/two_nodes.canlab.yaml
```

Terminal 2 — GUI:

```bash
cd frontend && npm install && npm run dev
```

Open the printed Vite URL, press Connect, then ▶ Run. The canvas shows the
loaded network, the scripted-traffic panel edits what Run transmits
(add/edit/delete), the analyzer streams frames (id/node filter, TX/RX
selector, pause/clear, click-to-inspect, CSV/JSON export). Protocol
details: `docs/api.md`.

## Run firmware from the GUI (Renode job table)

```bash
./firmware/tests/stm32_can/build.sh
export PATH="$PWD/.tools/dotnet:$PATH"   # only if your renode needs it (see above)
cargo run -q --bin canlab -- serve [--max-jobs 2]
```

In the *Renode firmware runs* panel: path
`firmware/tests/stm32_can/two_nodes.canlab.yaml`, budget `30`, Start.
Poll to `done`, then **Import trace** — the observed firmware frames
replay into the analyzer. Headless equivalent:
`canlab simulate firmware/tests/stm32_can/two_nodes.canlab.yaml --run-secs 30`.
See `docs/mcu-backends.md` (limits: no cancellation yet, newest 32
finished jobs kept, full UART logs are CLI-only).

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
