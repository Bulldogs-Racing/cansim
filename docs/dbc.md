# DBC support (PROMPT.md §38, Phase 12 slice 1)

Import a `.dbc` file's message/signal layout and decode Classical CAN
payloads into physical values. Parser subset: `VERSION`, `BU_` nodes,
`BO_` messages, non-multiplexed `SG_` signals (Intel `@1` and Motorola
`@0` layouts, signed/unsigned, factor/offset, ranges, units, receivers).

```bash
canlab dbc examples/vehicle.dbc --id 0x100 --data "00 10 50 0A 00 00 00 00"
```

```text
0x100 EngineData (4 signal(s)):
  Rpm = 512 rpm
  CoolantTemp = 40 degC
  Flags = 10
  BigSpeed = 0.1 km/h
```

## Rules

- Unknown sections (`NS_`, `BS_`, `CM_`, `BA_`, `VAL_`, …) are ignored by
  design; malformed `BO_`/`SG_` lines fail with file + line number.
- Multiplexed signals (`m0`/`M`) are rejected explicitly — flatten the
  multiplexer or wait for Phase 12 depth.
- Payloads shorter than the message DLC fail; longer ones decode their
  leading bytes (senders may pad).
- Implementation: `src/dbc.rs` (`Dbc::parse`, `Dbc::decode`), tested with
  hand-computed Intel + Motorola vectors (no copied reference data).

## GUI + WS decoding (slice 2)

`DecodeFrame { dbc, id, data }` decodes one payload against a
server-side `.dbc` path (relative to the serve cwd, like project paths)
and replies `DecodedSignals`; the frame inspector's **Decode with DBC**
button calls it for the selected row (path box defaults to
`examples/vehicle.dbc`). The file parses per request — DBC files are
small, and every failure (missing file, unknown id, short payload) is an
actionable `Error`.

## Deferred (explicit)

Multiplexing, `VAL_` tables, attributes/comments, extended multiplexing,
J1939 protocol decoding, DBC writing, and `.dbc`
references from project files.
