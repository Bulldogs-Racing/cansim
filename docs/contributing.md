# Contributing

This file exists so the `docs/` checklist in PROMPT.md §57 is complete;
the real guide lives at the repo root: [`CONTRIBUTING.md`](../CONTRIBUTING.md).

Short version: keep the layering (`src/can` ← `simulation` ← `project` ←
`backends` ← `server` ← `frontend`), test before tricky protocol code,
fail loudly instead of faking, and run `cargo fmt --check`,
`cargo clippy --all-targets`, `cargo test`, and `npm run typecheck`
before asking for review.
