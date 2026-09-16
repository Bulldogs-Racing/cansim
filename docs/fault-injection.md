# Fault injection (PROMPT.md §35–§36, Phase 9 slices 1–2)

Deterministic faults, two layers: single-shot ad-hoc frames, and seeded
project policies. No randomness anywhere — every fault names its exact
target or seed, so runs stay reproducible.

## Single-shot frames

Drive one corrupted frame now, via the GUI *Fault injection* panel or
over WS (`InjectFault`):

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

## Project policies (`faults:`)

Seeded probabilistic faults for whole runs (`examples/faults.canlab.yaml`):

```yaml
faults:
  - fault: DropFrame
    node: dashboard        # optional: only this sender (omit = all)
    id: 0x200              # optional: only this id (extended: true = 29-bit)
    probability: 0.5       # per-transmission, 0.0..=1.0
    seed: 42               # this rule's draw stream
```

Semantics:

- Every *normal* transmission (scripted, injected, imported, headless)
  checks the rules in order; the first matching rule draws from its own
  seeded stream and faults the frame on a draw below `probability`.
  Ineligible rules never advance their stream, so unrelated rules cannot
  perturb each other's draws.
- Same project + same operation order = identical event log, always
  (verified by record → record diff). `Reset` reseeds to the deterministic
  start; every load/edit/save rebuild reseeds too.
- Explicit single-shots (`InjectFault`, arbitration rounds) never draw.
- CLI timelines mark faulted lines (`[dropped, drove nothing]`,
  `[fault: CRC corrupted, observed CRC error]`).
- **Replay interaction**: replay disables policies and re-drives drops
  exactly (`FrameDropped` rows carry the fault), but transmitted-yet-
  errored frames (e.g. CRC) replay clean — their corruption is not
  recoverable from the trace. The replayed log therefore matches the
  record except for re-faulted deliveries.
- WS CRUD for policies is deferred: author them in the project file
  (validated like `messages:`); `GetProject` returns them and every Run
  honors them, GUI included.

Policies are editable over WS (`AddFault`/`UpdateFault`/`RemoveFault`,
index-addressed like messages — see `bugs.md`) and in the GUI *Fault
policies* panel; a node named by a policy cannot be removed first.

## Node disable/enable (§35 node faults, slice 4)

The properties panel's **Disable node** takes a node off the bus without
changing the topology: it neither drives nor receives, keeps its
declaration and error counters, and still counts for bus-in-use guards.
Transmitting *from* a disabled node fails loudly (like bus-off).
Disables are runtime-only — never written to the project file — and
**reset re-enables everything** (fresh deterministic start, alongside
reseeded fault streams). The header shows a ⛔ chip while any node is
disabled; enable/disable transitions are bus events in the log.

## Deferred (explicit)

- Bus-line faults (disconnect CANH/CANL, force dominant/recessive),
  node faults (force bus-off, disable, latency), durations, and the full
  Phase 9 UI matrix from §35.
- The `FlipBit` offset counts stuffed-wire bits (`SOF..EOF`); a future
  field-aware selector (SOF/arbitration/control/data/CRC/ACK/EOF) layers
  on top without changing these semantics.
