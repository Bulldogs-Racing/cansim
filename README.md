# CanLab — Visual CAN Network & MCU Simulator

CAN-first simulator: visually build networks of simulated STM32 / Arduino /
Teensy nodes, run firmware, and debug real CAN traffic. See `PROMPT.md` for
the full product spec and `docs/` for developer documentation.

> Status: **Phases 0–2 + headless Phase 3** — deterministic CAN core
> (frames, arbitration, virtual bus) with bit-level codec (CRC-15,
> stuffing, SOF..EOF), ACK handling, TEC/REC confinement, bus-off, and
> deterministic wire faults. `canlab simulate` runs without a GUI. Renode firmware execution (Phase 4) and the visual editor
> (Phase 5) are scaffolded, not yet implemented.

## Quickstart

```bash
cargo run -q --bin canlab -- simulate examples/two_nodes.canlab.yaml
cargo test
```

More: `docs/getting-started.md`, `docs/architecture.md`, `docs/can-model.md`.

## CLI

```bash
canlab new my-project                  # scaffold portable project dir
canlab simulate project.canlab         # headless deterministic run
canlab validate project.canlab         # schema + safety checks
canlab doctor                          # Renode / toolchains / SocketCAN
```

## Layout

```text
src/can/          CAN core (no emulator/GUI deps)
src/simulation/   deterministic clock, events, headless engine
src/project/      versioned YAML project format + validation
src/main.rs       CLI
tests/can_core.rs §74 acceptance tests
examples/         runnable projects
frontend/         Phase 5 React shell (placeholder)
docs/             architecture, CAN model, guides
```

## Contributing

Keep the layering (§3): CAN core ← simulation ← backends ← API ←
frontend. Small modules, tests before tricky protocol code, explicit
`…NotSupported` errors instead of silent fakes.
