# Known concerns & future-bug watchlist

Tracked worries that are not bugs today but will become bugs if the noted
conditions ever hold. Check here before building concurrent editors,
file-watching, or new execution paths.

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
