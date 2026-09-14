# CanLab architecture (Phase 0, PROMPT.md §3)

Layered design. Each layer only depends on the layers below it.

```text
┌───────────────────────────────────────────────┐
│                 Frontend                      │  frontend/ (React/TS shell; Phase 5)
│ React / TypeScript                            │
├───────────────────────────────────────────────┤
│              Application API                  │  deferred: local WebSocket API
│ Project management / simulation control       │
├───────────────────────────────────────────────┤
│            Simulation Manager                 │  src/simulation/ (Engine, SimClock)
├───────────────────────────────────────────────┤
│                CAN Core                       │  src/can/ (frame/id/bus/arbitration)
│ bus / arbitration / frames / timing / errors  │
├───────────────────────────────────────────────┤
│           MCU Backend Interface               │  deferred: trait (Phase 4)
├───────────────────────────────────────────────┤
│                  Renode                       │  deferred: external process (Phase 4)
│          Initial emulation backend            │
├───────────────────────────────────────────────┤
│             Future backends                   │
│ QEMU / native / other emulators               │
├───────────────────────────────────────────────┤
│             Hardware Interfaces               │  deferred: SocketCAN/USB-CAN (Ph. 13+)
│ SocketCAN / USB-CAN                           │
└───────────────────────────────────────────────┘
```

Rules (enforced in review, not just prose):

- The frontend never talks to Renode; it consumes the timestamped event
  stream (`SimEvent` / `BusEvent`) over the future WebSocket API.
- No Renode-specific assumption lives in `src/can/`. The CAN core compiles
  and tests with zero emulator dependencies.
- Three modeling levels stay separate (§64): frame (ID/DLC/DATA) →
  controller (bxCAN/FlexCAN/MCP2515) → physical layer (transceiver/CANH/L).
  The MVP implements the frame + logical-bus level only.

## Repository map

```text
src/can/          CAN core: id, frame, bus, arbitration, timing
src/simulation/   SimClock, SimEvent log, headless Engine
src/project/      versioned YAML project format + untrusted-input validation
src/backends/     McuBackend seam (api) + Renode supervised runs (renode)
src/main.rs       `canlab` CLI: new / simulate / validate / doctor / open
tests/can_core.rs §74 acceptance tests (arbitration + delivery)
tests/renode_backend.rs live Renode test (ignored by default, needs ELF + emulator)
examples/         runnable .canlab.yaml projects
backends/renode/  REPL overlay, .resc scripts (checked-in spike scripts)
firmware/tests/   bare-metal STM32F103 TX/RX fixtures + build.sh
frontend/         Phase 5 shell (types + placeholder canvas)
docs/             user + developer documentation
```

## Phase status

- [x] Phase 0 — workspace, project format, docs, CI
- [x] Phase 1 — CAN core + §74 tests
- [x] Phase 3 (headless part) — `canlab simulate` on virtual nodes
- [x] Phase 2 — CRC, stuffing, ACK, error counters, bus-off (+ wire faults)
- [x] Phase 4 (headless part) — Renode backend: real STM32F103 firmware runs
- [ ] Phase 4 (interactive part) — pause/step/inspect for the server layer
- [ ] Phase 5+ — visual editor, analyzer, MCP2515, Teensy, faults, FD, DBC
