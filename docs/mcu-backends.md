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

Verified live: two-node STM32F103 TX→RX exchange passes
(`cargo test --test renode_backend -- --ignored`), most recently against
Renode v1.17.0 + .NET 8.0.31 + ARM GCC 14.2.rel1, all repo-local except a
system `renode` package.

### Runtime prerequisite: dotnet

The Renode launcher execs `dotnet` itself, so Renode needs a .NET 8+
runtime even when the emulator binary is installed. `RenodeBackend::discover`
checks this up front (`DotnetMissing` / `DotnetTooOld` with install
instructions) and `canlab doctor` probes it — a present-but-unrunnable
Renode fails fast instead of dying mid-run inside the emulator child.

## Interactive runs (WS job table)

`canlab serve [--max-jobs N]` (default 1) runs firmware in the
background while the GUI stays interactive:

1. Build the fixture firmware (`./firmware/tests/stm32_can/build.sh`).
2. In the GUI's *Renode firmware runs* panel: path
   `firmware/tests/stm32_can/two_nodes.canlab.yaml`, run budget, Start.
3. The job table polls to `done`; **Import trace** replays the observed
   TX frames through the session engine so the CAN Analyzer shows
   firmware traffic.

Protocol: `StartRenodeRun` / `GetRenodeJob` / `CancelRenodeJob` /
`ListRenodeJobs` / `ImportRenodeTrace` (`docs/api.md`). A running job can
be cancelled (graceful emulator shutdown, `cancelled` state, partial
observations discarded — cancel early, import never). Semantics: jobs run
against project files (same contract as headless: all-`renode` nodes,
first bus, project-relative firmware, per-node pre-checks — shared
builder `spec_from_project` so CLI and GUI agree); import is batch-inject
at the current engine time and needs the session to contain the observed
senders. Limits (explicit): no per-job UART over WS (use headless
`simulate` for full logs), newest 32 finished jobs kept. Live coverage:
`cargo test --test renode_ws -- --ignored` (import + cancel).

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
