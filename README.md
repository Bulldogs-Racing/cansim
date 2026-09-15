# CanLab — Visual CAN Network & MCU Simulator

CAN-first simulator: visually build networks of simulated STM32 / Arduino /
Teensy nodes, run firmware, and debug real CAN traffic. See `PROMPT.md` for
the full product spec and `docs/` for developer documentation.

> Status: **Phases 0–2 + headless Phase 3 + Phase 4 (headless + WS job
> table) + Phase 5 (serve, canvas, topology editing, scripted-traffic
> editor) + Phase 6 slice 1 (analyzer depth)**
> — deterministic CAN core (frames, arbitration, virtual bus) with bit-level
> codec (CRC-15, stuffing, SOF..EOF), ACK handling, TEC/REC confinement,
> bus-off, and deterministic wire faults. `canlab simulate` runs virtual
> nodes without a GUI and runs real STM32F103 firmware in Renode
> (`backend: renode`, all-renode projects). `canlab serve` exposes the
> session over WebSocket (plus background firmware runs with trace import);
> the React frontend renders the topology canvas, edits it (palette,
> drag-and-drop, properties, save), edits scripted traffic (add/update/remove),
> runs firmware jobs, and shows a live CAN Analyzer (TX/RX filter,
> pause/clear, CSV+JSON export).

## Quickstart

```bash
cargo run -q --bin canlab -- simulate examples/two_nodes.canlab.yaml
cargo test
```

More: `docs/getting-started.md`, `docs/architecture.md`, `docs/can-model.md`.

## CLI

```bash
canlab new my-project                  # scaffold portable project dir
canlab simulate project.canlab         # headless run (virtual or Renode)
canlab simulate renode.canlab --run-secs 30 --export-json events.json
canlab serve --project project.canlab [--max-jobs 2]  # local WebSocket API for the GUI
canlab validate project.canlab         # schema + safety checks
canlab doctor                          # Renode / dotnet / toolchains / SocketCAN
```

## GUI

The GUI is a web app with three parts: a network **canvas**, run/simulation
**controls**, and a live **CAN Analyzer**.

### Prerequisites

- Rust (build `canlab` via `cargo build`)
- Node.js ≥ 18 (frontend dev server)

### Run it

1. **Start the API** from the repo root (relative project paths in the
   project file resolve here):

   ```bash
   cargo run -q --bin canlab -- serve --project examples/two_nodes.canlab.yaml
   ```

   Expected output: `CanLab API serving on ws://127.0.0.1:21011`.
   Keep this terminal open.

2. **Start the frontend** in a second terminal:

   ```bash
   cd frontend
   npm install      # first time only
   npm run dev
   ```

   Vite prints a URL (e.g. `http://localhost:5173`). Open it in a browser.

3. **Press Connect** (top-left). The status pill should change from `● idle`
   to `● running` after you press **Run**.

### Use it

| Button | Does |
|---|---|
| **Load** | Loads/validates the project in the path box; renders the topology on the canvas |
| **Run** | Executes the project's scripted traffic deterministically |
| **Pause / Resume** | Halts / continues the simulation |
| **Stop / Reset** | Ends the run or resets all nodes |
| **Step** | Advances simulated time without traffic |

- **Canvas**: buses are shown as blue nodes, MCUs as dark nodes, with
  animated edges for connections.
- **Editing**: the palette adds CAN buses and nodes (auto-named, attach to
  the first bus). Click a bus/node for the properties panel (bitrate,
  device, bus attachment, firmware path). Drag nodes to arrange; drag an
  edge from a node onto another bus to re-attach it; Delete key removes the
  selected node. **New** starts a blank canvas, **Save** writes the project
  YAML to the path box (save-as included); the ● unsaved chip tracks dirty
  state. Removing a bus in use or a node with scripted messages is refused
  with the reason shown.
- **Scripted traffic**: the panel below the canvas lists the frames Run
  transmits in order — add frames (sender, hex id, hex bytes, extended
  flag), edit rows in place (Edit loads a row into the form, Update
  applies it), delete rows. Form-level hex parsing catches typos inline;
  server-side sender/range/DLC validation is the backstop. Removing a
  node still referenced here is refused until its rows are deleted.
- **CAN Analyzer**: streams TX/RX frames as they happen (Time, Dir, Node,
  ID, DLC, Data). Filter by ID/node text plus a TX/RX direction selector,
  click a row to inspect the frame in the inspector, **Pause capture** to
  freeze the view (resume picks up the backlog), **Clear** to drop rendered
  rows, and **Export CSV/JSON** to save the trace. The table is capped
  at 500 rows.
- **Renode firmware runs**: start real STM32F103 firmware in the
  background (project path + run budget), watch jobs poll to done, then
  **Import trace** to replay observed firmware frames into the analyzer.
  Needs the fixture ELFs (`firmware/tests/stm32_can/build.sh`) and the
  dotnet runtime if your renode requires it (`canlab doctor` tells you).
- **Fault injection**: drive one corrupted frame now (flip a wire bit,
  corrupt the CRC, or drop the frame). Dropped frames show as DROP rows;
  the outcome line names the detection kind. Details:
  `docs/fault-injection.md`.
- The API repo note in-app always assumes `canlab serve` runs from the repo
  root.

Full protocol details: `docs/api.md`.

## Layout

```text
src/can/          CAN core (no emulator/GUI deps)
src/simulation/   deterministic clock, events, headless engine
src/project/      versioned YAML project format + validation
src/backends/     McuBackend seam + supervised Renode runs
src/main.rs       CLI
tests/can_core.rs §74 acceptance tests
tests/renode_backend.rs live Renode test (ignored; needs ELF + emulator)
backends/renode/  STMCAN overlay + .resc scripts
firmware/tests/   bare-metal STM32F103 TX/RX fixtures
examples/         runnable projects
frontend/         Phase 5 React shell (placeholder)
docs/             architecture, CAN model, guides
```

## Contributing

Keep the layering (§3): CAN core ← simulation ← backends ← API ←
frontend. Small modules, tests before tricky protocol code, explicit
`…NotSupported` errors instead of silent fakes.

## License

This software is distributed under the Beerware License (Revision 42). See
`LICENSE`. In short: do whatever you want with it; if we ever meet and you
think it was worth it, buy us a beer.
