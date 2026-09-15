# Contributing to CanLab

## Layering (§3 — enforced in review)

```
CAN core (src/can) ← simulation ← project ← backends ← server ← frontend
```

- `src/can/` compiles and tests with zero emulator/GUI dependencies.
  No Renode assumption lives there, ever.
- The GUI never simulates: it drives `canlab serve` over WebSocket.
- Small modules, documented public interfaces, no new dependencies
  without a reason written down.

## Quality bar (§72)

- Tests before tricky protocol code; run the full suite green.
- Explicit `…NotSupported` errors instead of silent fakes. If firmware
  can't execute on a device, say which phase unlocks it — never emulate
  it as something else.
- Never claim a feature works without testing it. No-emulator logic gets
  unit + WS tests; emulator logic gets an ignored live test (see below).

## Commands

```bash
cargo fmt --check && cargo clippy --all-targets && cargo test
cd frontend && npm run typecheck && npm test
./firmware/tests/stm32_can/build.sh
```

Live (ignored) tests need ELFs + Renode + the .NET runtime:

```bash
export PATH="$PWD/.tools/dotnet:$PATH"   # only if your renode needs it
cargo test --test renode_backend -- --ignored --nocapture   # ~60 s
cargo test --test renode_ws -- --ignored --nocapture        # ~60 s
```

## Commits

- Small, phase-named slices (`Phase 9 (slice 1): …`) with what was
  verified in the body (counts, live runs, fmt/clippy).
- Commit when asked or when a slice is done and green; never push unless
  asked. Leave other people's uncommitted changes alone.
- Docs ride with the code (`docs/*.md`, `README.md`, `bugs.md`
  watchlist for latent invariants).

## Repo graph

`AGENTS.md` describes the `graft/` context graph (also mirrored for
other assistants). After big code changes, refresh it with
`graft build`.
