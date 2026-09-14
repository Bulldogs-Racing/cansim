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
| `GetStatus` | `Status` | state, projectPath, buses, nodes, nextSeq. |
| `GetProject` | `Project` | Full project document for the canvas. |
| `Ping` | `Pong` | |

## Event stream (§52–§53)

`Events[i].seq === i` always; clients resume from `nextSeq`. `kind` is the
serialized `SimEventKind`: lifecycle markers plus `BusTraffic(...)` with
the full `CanFrame` (`id` as `{Standard: n}` / `{Extended: n}`, `dlc`,
`data` bytes). The frontend formats IDs (`idToHex`) and timestamps
(`fmtTimeNs`); the server never pre-formats.

## Scope notes

- No push broadcast yet — polling is the v1 contract and is covered by
  `tests/ws_api.rs` (real client ↔ real server over loopback).
- No Renode supervision over WS yet: firmware runs stay in
  `canlab simulate` (see `docs/mcu-backends.md`). The protocol already has
  room (`Status.state`, per-node backends in `Project`).
- No auth: loopback-only by design. Never bind this to a public interface
  without adding authentication first.
