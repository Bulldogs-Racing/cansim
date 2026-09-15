# Local API contract (PROMPT.md §4)

Source of truth: `src/server/proto.rs` (Rust) and `frontend/src/api.ts`
(TypeScript) — kept in sync by hand. This file is the overview, not the
spec.

## Transport

```bash
canlab serve --port 21011 [--project <file>]
```

- One WebSocket server on loopback, one authoritative `Session` shared by
  all connections behind a `Mutex`.
- One request → exactly one JSON reply. Every message is a tagged union on
  `type`; wire field names are camelCase on both sides.
- Binary frames are rejected; garbage is an `Error` reply, never a dropped
  connection.

## Requests

| Client sends | Server replies | Notes |
|---|---|---|
| `Load { path }` | `Status` | Validates + rebuilds the engine. Virtual backends only — Renode projects get an explicit error pointing at `canlab simulate`. |
| `Start` | `RunSummary { transmitted, received, nextSeq }` | Runs scripted `messages:` (or the demo frame) to completion, deterministically. |
| `Pause` / `Resume` / `Stop` / `Reset` | `Status` | Engine lifecycle. |
| `Step { deltaNs }` | `Stepped { nowNs, nextSeq }` | Advance simulated time without traffic. |
| `Inject { sender, id, extended?, data? }` | `Injected { receivers, nextSeq }` | One ad-hoc frame now. |
| `InjectFault { sender, id, extended?, data?, fault }` | `FaultInjected { error, receivers, nextSeq }` | One deterministically faulted frame now (`fault`: `{"FlipBit": offset}` / `"CorruptCrc"` / `"DropFrame"`). `error` names the detection (`Bit`/`Stuff`/`Crc`/`Form`/`Ack`) or null. See `docs/fault-injection.md`. |
| `GetEvents { sinceSeq? }` | `Events { events, nextSeq }` | Ordered log; `seq` is the index. The analyzer polls this (~500 ms) and appends rows into a capped ring buffer. |
| `GetStatus` | `Status` | state, projectPath, buses, nodes, nextSeq, dirty. |
| `GetProject` | `Project` | Full project document for the canvas. |
| `Ping` | `Pong` | |
| `NewProject` | `Status` | Blank unsaved canvas (projectPath null, dirty). |
| `AddBus { id, bitrate }` | `Status` | Adds a `type: can` bus. |
| `UpdateBus { id, bitrate }` | `Status` | Changes a bus bitrate. |
| `RemoveBus { id }` | `Status` | Refused while nodes attach to it. |
| `AddNode { node }` | `Status` | Full node declaration (`backend` must be `virtual` live). |
| `UpdateNode { id, node }` | `Status` | Replaces a declaration (`node.id` must equal `id` — renames refused). Powers canvas edge-drag re-attach and the properties panel. |
| `RemoveNode { id }` | `Status` | Refused while scripted `messages:` reference it. |
| `SaveProject { path? }` | `Status` | Strict-validates, writes YAML (`path` = save-as), clears dirty. |
| `AddMessage { message }` | `Status` | Appends a scripted frame (sender must exist; id range + DLC enforced). |
| `UpdateMessage { index, message }` | `Status` | Replaces the frame at `index`. |
| `RemoveMessage { index }` | `Status` | Deletes the frame at `index` (later rows shift — see `bugs.md`). |
| `StartRenodeRun { path, runSecs? }` | `RenodeJobStarted { jobId }` | Validates an all-`renode` project file and starts a supervised firmware run on a background thread (prompt reply, never blocks). `runSecs` defaults to 30. Refused when the `--max-jobs` running budget is exhausted. |
| `GetRenodeJob { jobId }` | `RenodeJob { job }` | One job: `state` (`running`/`done`/`failed`), TX/RX counts, `error`. |
| `ListRenodeJobs` | `RenodeJobList { jobs }` | All jobs, oldest first. The GUI polls this alongside events. |
| `ImportRenodeTrace { jobId }` | `TraceImported { transmitted, received, nextSeq }` | Replays a `done` job's observed TX frames through the session engine (batch-inject at current engine time) so the analyzer shows firmware traffic. Fails while `running`, surfaces the job error when `failed`, and requires the session to contain the observed senders. |

## Editing semantics

- Every successful edit rebuilds the engine from scratch, so the event log
  restarts at `nextSeq: 0`. Clients adopt the cursor from the `Status`
  reply and drop analyzer rows at or past it — the analyzer always shows
  the current engine's log, never a mix of two topologies.
- Edits are clone-mutate-rebuild-commit: failure leaves the session
  untouched, and every refusal is an `Error` reply naming the fix
  (duplicate id, bus in use, scripted message still referencing the node…).
- Intermediate states may be incomplete (e.g. no buses yet) — strict
  completeness is enforced by `SaveProject` (and `Load`), not by every
  keystroke. `Start` on a nodeless project explains itself (`NoNodes`).
- Canvas positions are view state only: they live in the frontend and are
  never written to the project file.

## Event stream (§52–§53)

`Events[i].seq === i` always; clients resume from `nextSeq`. `kind` is the
serialized `SimEventKind`: lifecycle markers plus `BusTraffic(BusEvent)`
with the full `CanFrame` (`id` as `{Standard: n}` / `{Extended: n}`, `dlc`,
`data` bytes). The frontend formats IDs (`idToHex`) and timestamps
(`fmtTimeNs`); the server never pre-formats.

Nesting (exact — the analyzer once read one level too shallow and showed
nothing; `tests/ws_api.rs` pins this shape):

```json
{ "seq": 3, "timeNs": 0,
  "kind": { "BusTraffic": {
    "time_ns": 0,
    "kind": { "FrameTransmitted": {
      "sender": "engine_ecu",
      "frame": { "id": { "Standard": 291 }, "dlc": 4, "data": [1, 2, 3, 4] } } } } } }
```

i.e. `event.kind.BusTraffic.kind` carries the variant, not
`event.kind.BusTraffic` directly.

## Analyzer behavior (Phase 6 slice 1, view-local)

Pause capture, Clear, the TX/RX direction selector, and CSV/JSON export
are pure frontend state — no protocol change:

- Pause freezes the `GetEvents` poll (cursor held); Resume re-polls from
  the held cursor, so no frames are lost, only deferred.
- Clear drops rendered rows; the server log is untouched.
- `UpdateMessage` is exposed in the scripted-traffic panel as in-place
  row editing (Edit → form → Update/Cancel).

## Scope notes

- No push broadcast yet — polling is the v1 contract and is covered by
  `tests/ws_api.rs` (real client ↔ real server over loopback).
- Renode jobs run on background threads against project *files*,
  independent of the loaded session: the session may hold a virtual
  project under edit while firmware runs. At most `--max-jobs`
  (`canlab serve --max-jobs N`, default 1) run concurrently; only the
  newest 32 finished records are kept (older polls/imports report an
  explicit unknown-job error). Full UART logs stay server-side — the CLI
  remains the place for them (`docs/mcu-backends.md`). Live coverage:
  `tests/renode_ws.rs` (ignored; needs ELFs + emulator + runtime).
- No auth: loopback-only by design. Never bind this to a public interface
  without adding authentication first.
