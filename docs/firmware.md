# Firmware guide

How to build firmware that runs in CanLab — on real emulated MCUs via
Renode today, on virtual nodes by reference.

## Fixture firmware (STM32F103, bare metal)

```
firmware/tests/stm32_can/
├── main.c        # TX role (-DROLE_TX) and RX role (-DROLE_RX)
├── startup.s     # vector table + reset handler
├── linker.ld     # flash@0x08000000 / ram@0x20000000
├── build.sh      # builds build/can_tx.elf + build/can_rx.elf
└── two_nodes.canlab.yaml   # all-renode project using those ELFs
```

```bash
./firmware/tests/stm32_can/build.sh   # needs arm-none-eabi-gcc (see below)
```

`build.sh` uses `../../../.tools/arm-gcc` by default; override with
`CANLAB_ARM_GCC=/path/to/arm-none-eabi-gcc`. Build output (`build/`) is
gitignored — rebuild after cloning.

## Toolchain

Any `arm-none-eabi-gcc` that targets Cortex-M3 works (verified: ARM GNU
14.2.rel1). Repo-local install, no root:

```bash
# unpack an x86_64 arm-none-eabi tarball from developer.arm.com, then:
ln -s <unpacked>/ .tools/arm-gcc   # or set CANLAB_TOOLS
```

`canlab doctor` reports the toolchain it finds.

## Writing bxCAN firmware that runs here

Constraints learned the hard way (see `docs/phase4-spike.md`):

- **Clock**: HSI 8 MHz, no PLL. PCLK1 = 8 MHz → USART2 BRR `0x451`
  (115200), bxCAN BTR `0x001C0000` (500 kbit/s, 16 TQ). Never init the PLL.
- **RCC is SVD-stubbed**: set the APB1/APB2 enable bits and proceed —
  never poll ready flags.
- **CAN init**: enter init mode via INRQ and wait for INAK (bounded
  waits; print `CAN INIT FAIL` and stop on timeout — the CLI surfaces it).
- **Filters**: accept-all filter bank 0 → FIFO0 is the proven shape.
- **Polling only**: TSR/RF0R polling, no interrupts in the fixtures.
- **UART report protocol**: the outcome parser (`backends::api`) reads
  USART2 text. Boot lines (`CANLAB TX BOOT`), `CAN INIT OK`, then per
  frame exactly `CAN TX OK 0x123 [01 02 03 04 05 06 07 08]` (TX) and
  `CAN RX 0x123 [...]` (RX). Status lines (`CAN RX WAIT`, `CAN RX DONE
  got=N`, `CAN TX DONE`) are status, never frames. Keep these formats or
  update the parser with them.

## Referencing firmware from projects

```yaml
nodes:
  - id: engine_ecu
    device: stm32f103
    backend: renode
    firmware: ./build/can_tx.elf   # project-relative, no "..", no absolutes (§21)
```

Missing files warn at validation and fail fast at run time with the node
name attached. Huge binaries are referenced, never embedded (§48).

## Importing Arduino sketches (static analysis, never compiled or run)

Write firmware in the Arduino IDE as usual, then extract its CAN intent:

```bash
canlab sketch firmware/examples/arduino/mcp_can_sender.ino --as engine_ecu
```

```text
firmware/examples/arduino/mcp_can_sender.ino: detected mcp_can

line  lib          id       ext  dlc  data
23    mcp_can      0x100    -    8    ?counter++ 02 03 04 05 06 07 08

messages:
  # TODO line 23: dynamic payload skipped (fill example bytes by hand)
```

The importer reads `#include` lines to pick a dialect at runtime
(`<mcp_can.h>` → `sendMsgBuf`, `<CAN.h>` → `beginPacket`/`write`/
`endPacket`; pin with `--dialect mcp_can|arduino-can`), resolves
same-file `#define`/`const`/byte-array initializers, and flags anything
runtime-computed (`?expr`) instead of inventing bytes. Fully constant
sends materialize into `messages:` rows with `source: file:line`
provenance; dynamic ones need hand-filled example bytes first.
`<FlexCAN_T4.h>` is recognized but deferred with an explicit error
(Phase 8); unknown libraries error naming the supported set. Example
fixtures live in `firmware/examples/arduino/` (parser-only, never
compiled). GUI import arrives next; the engine never changes — imports
are ordinary scripted messages.
