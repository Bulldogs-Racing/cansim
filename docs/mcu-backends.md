# MCU backends

## Backend interface (PROMPT.md §17, §56)

```text
McuBackend
 ├── run()          // supervised run -> ground-truth observations
 ├── name()
 └── (pause/step/inspect: future server layer, not faked here)
```

The frontend never knows which backend is active. Per-device support is a
matrix, never an assumption (`STM32 ≠ Renode`):

```text
STM32F103 ──┬── Renode (works: see below)
            ├── future QEMU
            └── future native
```

## Current status (this build)

| Backend   | Status                                                        |
|-----------|---------------------------------------------------------------|
| `virtual` | Runs now: deterministic logical-frame nodes for CI/headless.  |
| `renode`  | Runs now for `stm32f103`: supervised emulator runs via `canlab simulate`. |

`canlab simulate` dispatches on the project `backend:` field: all-virtual
runs use the logical engine as before; all-renode runs boot real firmware
in Renode (one machine per node, CAN1s joined on a CAN hub), capture UART
output, and replay observed TX frames through the deterministic engine so
CLI output and `--export-json` share the virtual-run format. Mixed
virtual+renode projects are rejected with an explicit error (no silent
cross-backend time sync).

```bash
canlab simulate project.canlab [--run-secs 30] [--export-json events.json]
```

Firmware is copy/paste: build it with your own toolchain (e.g.
`arm-none-eabi-gcc -mcpu=cortex-m3`), reference the `.elf` from the
project file with a project-relative path. No IDE integration needed.

### Renode run semantics (read this once)

- The `--run-secs` budget (default 30) is the *duration* of the firmware
  run, not a failure timeout: firmware loops forever by nature, so the
  manager shuts Renode down gracefully (`quit` on the monitor, kill
  fallback — never orphaned) when the budget elapses and reports what was
  observed.
- Observed TX frames are replayed through `CanBus`/`Engine` for
  deterministic timestamps; firmware UART lines print verbatim as ground
  truth. Reported RX lines are cross-checked against observed TX.
- A lone transmitting node still reports `TXOK` (bxCAN/model leniency) —
  delivery requires a second node on the hub. The CLI says so instead of
  implying success.
- Only `stm32f103` executes. `arduino_uno` (no AVR core in Renode,
  Phase 7) and `teensy41` (Renode's i.MX RT1062 platform labels but does
  not model FlexCAN, Phase 8) fail with named errors — never silently as
  an STM32 (§72).

## Renode plan (Phase 4, PROMPT.md §18–§19)

Done for headless runs: external-dependency treatment (managed process,
generated `.resc`, no forks, unique temp dirs, no hard-coded paths),
startup/missing-install/crash/invalid-firmware handling with actionable
errors, timeout-as-duration, clean shutdown. Remaining: interactive
control (pause/step/inspect) for the future server/WebSocket layer.
See `docs/phase4-spike.md` for the proving spike and the Renode scripting
quirks the generator encodes.
