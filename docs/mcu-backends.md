# MCU backends

## Backend interface (planned, Phase 4)

```text
McuBackend
 ├── load_firmware()
 ├── start() / stop() / reset() / pause() / resume() / step()
 ├── inspect()
 └── peripherals()
```

The frontend never knows which backend is active. Per-device support is a
matrix, never an assumption (`STM32 ≠ Renode`):

```text
STM32F103 ──┬── Renode (Phase 4)
            ├── future QEMU
            └── future native
```

## Current status (this build)

| Backend   | Status                                                        |
|-----------|---------------------------------------------------------------|
| `virtual` | Runs now: deterministic logical-frame nodes for CI/headless.  |
| `renode`  | Validates now; simulation reports it as pending Phase 4.      |

`renode` projects author and validate cleanly (with a warning), so
topologies are ready before the emulator lands — but nothing pretends to
emulate. A Teensy 4.1 node is likewise recognized and runs as a generic
CAN node with a warning, never silently as an STM32 (§72).

## Renode plan (Phase 4, PROMPT.md §18–§19)

- Treat Renode as an external dependency: managed process, generated
  `.resc` scripts/platform descriptions, no forks, no hard-coded temp paths.
- Handle startup failure, missing installs, crashes, timeouts, invalid
  firmware — with actionable UI errors and never orphaned processes.
