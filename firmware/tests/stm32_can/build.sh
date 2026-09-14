#!/usr/bin/env bash
# Build the Phase 4 spike firmware with the repo-local ARM toolchain.
# Outputs: build/can_tx.elf build/can_rx.elf (+ .bin for inspection).
set -euo pipefail
cd "$(dirname "$0")"

GCC="${CANLAB_ARM_GCC:-../../../.tools/arm-gcc/bin/arm-none-eabi-gcc}"
OBJCOPY="$(dirname "$GCC")/arm-none-eabi-objcopy"
OBJDUMP="$(dirname "$GCC")/arm-none-eabi-objdump"

CFLAGS="-mcpu=cortex-m3 -mthumb -O1 -g -ffreestanding -nostdlib -nostartfiles \
  -Wall -Wextra -Werror -ffunction-sections -fdata-sections"
LDFLAGS="-T linker.ld -Wl,--gc-sections -Wl,-Map=build/can.map"

mkdir -p build
$GCC $CFLAGS -DROLE_TX -c main.c -o build/tx.o
$GCC $CFLAGS -c startup.s -o build/startup.o
$GCC $CFLAGS build/startup.o build/tx.o $LDFLAGS -o build/can_tx.elf
$GCC $CFLAGS -DROLE_RX -c main.c -o build/rx.o
$GCC $CFLAGS build/startup.o build/rx.o $LDFLAGS -o build/can_rx.elf
$OBJCOPY -O binary build/can_tx.elf build/can_tx.bin
$OBJCOPY -O binary build/can_rx.elf build/can_rx.bin

echo "--- TX ---"
$OBJDUMP -h build/can_tx.elf | head -n 12
echo "--- RX ---"
$OBJDUMP -h build/can_rx.elf | head -n 12
echo "build ok: build/can_tx.elf build/can_rx.elf"
