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
src/server/       Session manager + Renode job table + JSON protocol + `canlab serve` (WebSocket)
src/main.rs       `canlab` CLI: new / simulate / validate / doctor / open / serve
tests/can_core.rs §74 acceptance tests (arbitration + delivery)
tests/renode_backend.rs live Renode test (ignored by default, needs ELF + emulator)
tests/ws_api.rs   WS contract test (real client ↔ server over loopback)
examples/         runnable .canlab.yaml projects
backends/renode/  REPL overlay, .resc scripts (checked-in spike scripts)
firmware/tests/   bare-metal STM32F103 TX/RX fixtures + build.sh
frontend/         React + React Flow editor: canvas, sim controls, analyzer, scripted-traffic editor, Renode job panel
docs/             user + developer documentation
```

## Phase status

- [x] Phase 0 — workspace, project format, docs, CI
- [x] Phase 1 — CAN core + §74 tests
- [x] Phase 3 (headless part) — `canlab simulate` on virtual nodes
- [x] Phase 2 — CRC, stuffing, ACK, error counters, bus-off (+ wire faults)
- [x] Phase 4 (headless part) — Renode backend: real STM32F103 firmware runs
- [x] Phase 5 (slice 1) — serve API + canvas view + run controls + analyzer
- [ ] Phase 4 (interactive part) — Renode supervision over WS + debugger integration
- [x] Phase 5 (slice 2) — topology editing: palette, drag-and-drop, save
- [x] Phase 5 (slice 3) — scripted-messages editor (add/update/remove over WS + panel, incl. in-place edit UI)
- [x] Phase 6 (slice 1) — analyzer depth: TX/RX filter, pause/clear capture, CSV+JSON export (view-local, no protocol change)
- [x] Phase 6 (slice 2) — analyzer sorting (newest/oldest) + structured ID filter (exact/range/mask, harness-verified). Arbitration-loss rows deferred: no live path emits simultaneous rounds yet (§34 needs engine exposure first)
- [x] Phase 4 (WS part, slice 1) — Renode job table over WS: StartRenodeRun/GetRenodeJob/ListRenodeJobs/ImportRenodeTrace, `--max-jobs` (default 1), trace import into the session engine (live-verified: `tests/renode_ws.rs`)
- [x] Phase 9 (slice 1) — deterministic single-shot faults end to end: FlipBit/CorruptCrc/DropFrame via engine → session → WS InjectFault → GUI panel, DROP analyzer rows, `docs/fault-injection.md` (probabilistic policies deferred)
- [x] Observability slice — `canlab replay`: deterministic headless re-run of `--export-json` traces (bit-identical logs, verified by replay-of-replay diff)
- [x] Export slice — PCAP export (`DLT_CAN_SOCKETCAN`, simulated timestamps, EFF/RTR flags incl. RTR-aware analyzer rows + inspector), verified by byte-level parse-back
- [x] Phase 12 (slice 1) — DBC subset: message/signal parsing + Intel/Motorola decoding + `canlab dbc` CLI (`docs/dbc.md`, `examples/vehicle.dbc`; multiplexing deferred)
- [x] Portability slice — `canlab package`: validated shareable closures (firmware + dbc/assets, missing firmware is fatal, revalidates from its new home)
- [x] Phase 4 (WS part, slice 2) — job cancellation (flag polled by the emulator thread; graceful shutdown, `cancelled` state, partials discarded; live-verified) + capped per-job UART over WS (`GetRenodeLog`, last-200 bound, GUI Log viewer; live-verified). Still deferred: Renode console/debugger (§50–§51)
- [ ] Phase 7 — accuracy-first MCP2515 virtual execution + SPI timing
- [ ] Phase 6+ — analyzer sorting/search, Teensy, faults, FD, DBC
