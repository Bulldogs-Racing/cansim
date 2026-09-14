# Phase 4 spike report: real STM32F103 CAN egress in headless Renode

Status: **proven**. Real bare-metal firmware on two simulated STM32F103s
exchanges Classical CAN frames through Renode's bxCAN model + CAN hub.
`cargo test` stays green (76/76); `src/` untouched.

## Result

```
tx-node/usart2: CANLAB TX BOOT
tx-node/usart2: CAN INIT OK
rx-node/usart2: CANLAB RX BOOT
rx-node/usart2: CAN INIT OK
tx-node/usart2: CAN TX OK 0x123 [01 02 03 04 05 06 07 08]   (x5)
rx-node/usart2: CAN RX 0x123 [01 02 03 04 05 06 07 08]      (x5)
rx-node/usart2: CAN RX DONE got=5
tx-node/usart2: CAN TX DONE
```

5/5 frames delivered with exact ID + payload. Reproduced twice: once via
interactive monitor, once standalone via the checked-in hub script.

## How to reproduce

```bash
# 1. Build firmware (repo-local toolchain, no system install)
./firmware/tests/stm32_can/build.sh

# 2. Run two-node exchange, headless (run from .tools/renode)
cd .tools/renode
timeout 90 ./renode --disable-xwt --port 0 \
  -e 'include @<repo>/backends/renode/scripts/canlab_stm32f103_hub.resc' \
  -e 'start' 2>&1 | grep -E 'CAN (TX|RX|INIT)'
```

Single-node TX-only variant: `backends/renode/scripts/canlab_stm32f103_single.resc`
(+ `sysbus LoadELF @<abs path to can_tx.elf>` before `start`).

## What was built (spike only, no backend manager yet)

| File | Purpose |
|---|---|
| `backends/renode/templates/stm32f103_can.repl` | Documented STMCAN overlay snippet |
| `backends/renode/scripts/canlab_stm32f103_{single,hub}.resc` | Headless boot + CAN-hub wiring |
| `firmware/tests/stm32_can/{startup.s,linker.ld,main.c,build.sh}` | Bare-metal TX/RX firmware, `-DROLE_TX` / `-DROLE_RX` |

## Key findings for the backend manager (Phase 4 proper)

1. **Overlay works, no fork needed.** Stock `stm32f103.repl` has no CAN;
   `machine LoadPlatformDescriptionFromString "can1: CAN.STMCAN @ sysbus
   <0x40006400, +0x400> { [0-3] -> nvic@[19-22] }"` exposes a working
   bxCAN at the manual-specified base/IRQ. Mirrors `stm32f4.repl`.
2. **Negative control passes.** Without the overlay, `CAN_MSR` reads return
   SVD stub zeros, `INAK` never sets, firmware prints `CAN INIT FAIL` and
   exits cleanly (bounded waits work). So `CAN INIT OK` evidences the real
   model, not the stub.
3. **RCC is SVD-stubbed, must not be waited on.** `RCC:APB2ENR/APB1ENR`
   log "unimplemented register ... returning SVD 0" warnings; enable-bit
   writes are accepted. Firmware sets enables and proceeds without polling
   ready flags — the manager's firmware guidance must say the same.
4. **Clock assumption documented in firmware:** HSI 8 MHz, no PLL ->
   PCLK1 = 8 MHz; USART2 BRR `0x451` (115200), bxCAN BTR `0x001C0000`
   (500 kbit/s, 16 TQ). No PLL init attempted.
5. **Renode scripting quirks (manager must generate accordingly):**
   - `.resc` comments are `#`, not `//`.
   - Peripheral load is `sysbus LoadELF`, not `machine LoadELF`.
   - `-e` commands need the `@` prefix for literal local paths, but
     `$VAR`/`$ORIGIN` paths are used WITHOUT `@`; `@$ORIGIN/...` and
     `@...` in `$var?=` defaults do NOT resolve and hang in a download
     retry. Working form: `$tx_elf?=$ORIGIN/...` + `sysbus LoadELF $tx_elf`.
   - `mach create "<name>"` makes the new machine current; subsequent
     `machine`/`sysbus`/`connector` lines apply to it. `connector Connect
     sysbus.can1 canHub` per machine; `emulation SetGlobalQuantum
     "0.000025"` + `SetGlobalSerialExecution True` keep nodes in sync
     (copied from Renode's NXP_FlexCAN/MCAN robot tests).
   - SVD is fetched from `dl.antmicro.com` on first use, then cached.
     Offline reruns work; a fresh machine without cache needs network once.
6. **Single-node TXOK is lenient.** A lone node reports `TXOK` with nobody
   to ACK. The graded proof is hub delivery to a second node, not TXOK
   alone — the manager's health check should assert RX-side observation.

## Scope kept (§72)

- Polling-only firmware, no interrupts; accept-all filter bank 0 -> FIFO0.
- One bus, 500 kbit/s, standard ID `0x123`, 8-byte payload.
- No `src/` changes, no trait/manager, no GUI. `renode` backend still
  validates-with-warning until the manager lands.
- Build artifacts (`firmware/**/build/`) are gitignored; ELFs rebuild via
  `build.sh` (`CANLAB_ARM_GCC` overrides the toolchain path).

## Recommended next step

Build the Rust backend manager on this evidence: `McuBackend` trait,
Renode child-process supervision (no orphans, timeouts, actionable
§68 errors), generated `.resc` from the project file (buses/nodes/firmware
paths), UART/CAN event capture into `SimEvent`, and `canlab simulate`
dispatch (`virtual` vs `renode`). The resc-generation rules in finding 5
are the spec for the generator.
