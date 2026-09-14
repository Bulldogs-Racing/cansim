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
- Frame duration is stuffing-exact: `(stuffed SOF..EOF bits + 3 IFS) /
  bitrate` (`bits::wire_duration_ns`, the clock step the engine uses).
  `CanFrame::nominal_*` remain as planning estimates only.

## Bit model (§10–§11)

- `bits::encode_frame` / `decode_frame` implement the full `SOF … EOF`
  layout for standard and extended data/remote frames; stuffing covers
  `SOF` through the CRC sequence; CRC/ACK delimiters and EOF are
  fixed-form; the ACK slot accepts either level and reports `acked`.
- CRC-15 (`x^15+…+1`, `0x4599`, init 0) over destuffed `SOF … data`,
  cross-validated against the `crc` crate catalogue implementation.
- The decoder maps raw DLC nibbles 9–15 to 8 data bytes (ISO DLC table);
  the encoder only emits 0–8. Reserved bits and SRR are accepted at
  either level (real receivers ignore them).

## Error handling (§12)

- Detection kinds: bit, stuff, CRC, form, ACK (`CanErrorKind`); error
  frames modelled as 6-bit flag + 8-recessive delimiter (`ErrorFrame`).
- Per-controller TEC/REC with Active (≤127) / Passive (≥128) / BusOff
  (TEC ≥ 256) states, exposed as `ControllerStatus` for the UI/analyzer.
- Counting: TX error +8 (error-passive + pure ACK error parks, so a lone
  node never ACKs itself bus-off), RX error +1, TX success −1, RX success
  −1 (127 snap above 127). Bus-off recovers after 128×11 recessive bits
  (`note_idle_11`) or CPU re-init (`reset_node`).
- Bus-off controllers neither drive, receive, nor acknowledge. No
  automatic retransmission yet — outcomes report `acked`/`error` and the
  retry policy belongs to the controller layer.
- `CanBus::transmit_with_fault` (`FlipBit` / `CorruptCrc` / `DropFrame`)
  is the deterministic bus-level fault hook; probabilistic Phase 9
  policies select offsets on top of it.

## Deferred (documented, not silent)

1. The +8 special cases around error/overload-flag bit sequences,
   overload frames, and the error-passive TX-suspend delay.
2. The transceiver/physical layer (§64–§65: CANH/CANL, termination,
   voltages); the bus operates at the logical frame level.

## Event stream (§52–§53)

Every `BusEvent` carries `time_ns`. Engine lifecycle (`SimulationStarted`
/ `Paused` / `Stopped` / `Reset`) wraps bus traffic in `SimEvent`, giving
one ordered stream for the CLI today and the WebSocket frontend later.
