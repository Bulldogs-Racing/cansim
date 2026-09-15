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
serialized `SimEventKind`: lifecycle markers plus `BusTraffic(...)` with
the full `CanFrame` (`id` as `{Standard: n}` / `{Extended: n}`, `dlc`,
`data` bytes). The frontend formats IDs (`idToHex`) and timestamps
(`fmtTimeNs`); the server never pre-formats.

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
- No Renode supervision over WS yet: firmware runs stay in
  `canlab simulate` (see `docs/mcu-backends.md`). The protocol already has
  room (`Status.state`, per-node backends in `Project`).
- No auth: loopback-only by design. Never bind this to a public interface
  without adding authentication first.
