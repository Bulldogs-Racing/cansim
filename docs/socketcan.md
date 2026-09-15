# SocketCAN bridging (PROMPT.md §39–§40, Phase 13 — deferred)

Intended architecture (not implemented):

```text
CanLab  →  SocketCAN (vcan0/can0)  →  candump / cansend / Linux CAN apps
simulated CAN bus  ↕  SOCKETCAN  ↕  USB CAN adapter  ↕  real CAN bus
```

Design constraints for when it lands (§41 safety):

- Core simulation never requires SocketCAN; the bridge is an adapter on
  the side of the engine, like the Renode backend.
- Physical transmission needs explicit user confirmation, a visible
  hardware indicator, and a prominent disconnect — never silent TX.
- Accidental project startup must not transmit unless configured.

Current status: `canlab doctor` only probes for `can0`/`vcan0`
presence. No bridge code exists; `src/` has no socket or kernel
dependency. Build the virtual/stable core first (Phases 4–12) — the
bridge is a thin adapter once replay/import/export are solid.
