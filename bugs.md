# Known concerns & future-bug watchlist

Tracked worries that are not bugs today but will become bugs if the noted
conditions ever hold. Check here before building concurrent editors,
file-watching, or new execution paths.

## Emulator shutdown must kill the process group, not the child

`run_supervised` spawns Renode in its own group (`setsid` at spawn);
`shutdown` kills by `killpg` and joins pipe drainers bounded. Killing
only the direct child orphans launcher grandchildren (`bash renode` ->
`dotnet Renode.dll`), which inherit the pipes and hang the drainers in
`read()` forever — this was the cancel-during-boot hang (job "running"
forever). Pinned by `kill_path_reaps_launcher_grandchildren` (stub
launcher + `pgrep` orphan check). Do not "simplify" back to
`child.kill()` + unbounded `join`.

## Analyzer cursor: fetch-then-adopt

`Start` / `Inject` / `ImportRenodeTrace` append events *before* the client
reads `Status.nextSeq`. The GUI must `GetEvents` since the pre-command
cursor *before* adopting the grown `Status` cursor (`runCmd`/`renodeCmd`
poll first, adopt second) — adopting first skips exactly the frames just
produced and the analyzer stays empty. This bit us twice (empty table
after Run even with correct parsing); `tests/ws_api.rs` pins the
server side (frames sit between pre-start cursor and post-start head).

Becomes a real bug again if any new command both appends events and
returns `Status` (or a cursor) that a client adopts before fetching.
Rule: fetch since the old cursor first, adopt second — or split the
reply so growth and cursor-adoption cannot be reordered. Corollaries in
`App.tsx`: while capture is paused `applyStatus` must not advance/prune
(else a paused Run's frames are lost on resume), while engine-rebuilding
edits must reset the cursor even when paused (else the rebuilt log,
restarted at 0, sits forever below a stale cursor).

## Scripted-message addressing is index-based

`UpdateMessage` / `RemoveMessage` take a list index, so rows shift after
deletes. Safe today because the server serializes all requests through one
lock (`serve.rs` dispatch holds the session `Mutex`) and the GUI refreshes
after every op.

Becomes a real bug if any of these ever exist:

- concurrent editors (second tab, automation script driving the API),
- file-watching auto-reload of external YAML edits,
- undo/redo in the frontend.

Fix then: add a stable `id` to `MessageDecl` (`src/project/schema.rs`) with
migration/defaulting for old files, and address ops by it.

Logged during Phase A planning; revisit before building any of the above.
