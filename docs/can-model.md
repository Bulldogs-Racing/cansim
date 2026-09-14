# CAN model (PROMPT.md §6–§12)

## Frame (Level 1)

`CanFrame { id: CanId, is_remote, dlc, data, format }`:

- `CanId::Standard(u16)` — 11-bit, `0x000..=0x7FF`.
- `CanId::Extended(u32)` — 29-bit, `0x00000000..=0x1FFFFFFF`.
- Classical CAN only: `dlc` 0..=8, data frames carry `data.len() == dlc`,
  remote (RTR) frames carry no payload and `dlc` is the requested length.
- `CanFormat::Fd` exists as a typed placeholder; constructing an FD frame
  returns `CanFrameError::FdNotSupported` (§37 — explicit, never silent).

Invalid states are unrepresentable: out-of-range IDs, `dlc > 8`, and
oversized payloads fail at construction.

## Bus + arbitration (logical level)

- `CanBus` registers/unregisters nodes, accepts transmissions, resolves
  simultaneous requests with real arbitration (lowest ID wins; standard
  beats extended on equal base IDs; data beats remote on equal IDs —
  ISO 11898-1 ordering), delivers the winning frame to every other node,
  and records `ArbitrationLost` for losers.
- Timestamps are integer nanoseconds, monotonic per bus. The bus never
  reads the wall clock; determinism falls out of the design (§9).
- Nominal frame duration = `nominal_bit_len × 1e9 / bitrate`, where
  `nominal_bit_len` counts SOF/ID/control/data/CRC/ACK/EOF/IFS **without**
  stuffing bits.

## Protocol assumptions (to revisit with the §10 bit model)

1. Raw DLC values 9..=15 are rejected (real hardware maps them to 8 data
   bytes on the wire). A future bit-level decoder will map them; the frame
   API stays strict for clarity.
2. CRC/stuffing/ACK/error counters/bus-off (§11–§12) are **not modeled
   yet**. The event stream (`BusEventKind`) is the extension point: error
   frames and counter updates will be new variants, not delivery changes.
3. The transceiver/physical layer (§64–§65: CANH/CANL, termination,
   voltages) is deferred; the bus operates at the logical frame level.

## Event stream (§52–§53)

Every `BusEvent` carries `time_ns`. Engine lifecycle (`SimulationStarted`
/ `Paused` / `Stopped` / `Reset`) wraps bus traffic in `SimEvent`, giving
one ordered stream for the CLI today and the WebSocket frontend later.
