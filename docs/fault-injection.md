# Fault injection (PROMPT.md §35–§36, Phase 9 slice 1)

Deterministic, single-shot wire faults. No randomness, no probabilities
yet — every fault names its exact target, so runs stay reproducible.

## What exists today

Three fault kinds, implemented at the bus level (`CanBus::transmit_with_fault`)
and exposed through the whole stack (engine → session → WebSocket → GUI panel):

| Fault | Effect | Analyzer shows |
|---|---|---|
| `FlipBit(offset)` | Flips the wire bit at 0-based `SOF..EOF` offset | TX row + RX rows if it still decodes, else nothing delivers and TEC/REC move |
| `CorruptCrc` | Flips a CRC-sequence bit; receivers always see a CRC error | No delivery; sender TEC += 8, receivers REC += 1 |
| `DropFrame` | Drives nothing; the frame never reaches the bus | A `DROP` row (`FrameDropped` event) |

Out-of-range `FlipBit` offsets fail as `FaultOffsetOutOfRange` — never
silently clamped. Unknown senders and bad frames fail like `Inject`.

## Use it

GUI *Fault injection* panel (sender, hex id/data, fault kind, bit offset
for flips), or over WS directly:

```json
{"type": "InjectFault", "sender": "engine_ecu", "id": 291,
 "data": [1, 2, 3, 4], "fault": {"FlipBit": 30}}
{"type": "InjectFault", "sender": "engine_ecu", "id": 291,
 "data": [1, 2], "fault": "CorruptCrc"}
{"type": "InjectFault", "sender": "engine_ecu", "id": 291,
 "data": [9], "fault": "DropFrame"}
```

Replies carry the ground truth: `FaultInjected { error, receivers,
nextSeq }`, where `error` is the detection kind (`Bit`/`Stuff`/`Crc`/
`Form`/`Ack`) or null when the frame decoded clean (or was dropped).

## Deferred (explicit)

- Probabilistic policies (`probability`/`duration`/`seed`, §36), bus-line
  faults (disconnect CANH/CANL, force dominant/recessive), node faults
  (force bus-off, disable, latency), and the Phase 9 UI matrix from §35.
- The `FlipBit` offset counts stuffed-wire bits (`SOF..EOF`); a future
  field-aware selector (SOF/arbitration/control/data/CRC/ACK/EOF) layers
  on top without changing these semantics.
