# Sideline reliability upgrade — approved scope

User approved the last proposed focused iteration (not every optional feature in the review).

## Principles and scope
Keep the small Tauri/no-framework offline app, existing task JSON array shape and Windows AppBar behavior. Do not add cloud/network, reminders, search, global shortcuts, monitor selection, new occupancy mode, or unrelated visual redesign. Preserve unknown user data rather than silently replacing it. Back up old sources and desktop executable before work.

## Acceptance / compact spec
A1. Editing a note only changes the selected task's note and editor appears in that task.
A2. Composing Chinese IME Enter does not add/commit task or note; ordinary Enter commits, Escape cancels.
A3. Note text/editor interaction cannot activate drag. Pending tasks remain reorderable.
A4. Serialized frontend writes, visible saving/saved/error + retry, and exit waits for pending writes; a failure prevents silent exit. Backend writes temp + flush + atomic replace, keeps last valid .bak, serializes reads/writes. Missing file is first use; malformed/unreadable existing file is an error, not an empty list. No writes until a successful load/recovery. Explicit recovery preserves corrupt original and restores valid backup.
A5. Delete task/note offers Undo (last deletion, button and Ctrl+Z outside text editors). Undo restores full task/notes and position; normal text editor undo is unaffected. No timed forced loss needed.
A6. Completed tasks render in a collapsible bottom section with count; checking/unchecking moves between sections without discarding notes. Drag indexes must remain original data indexes. Completed-section expansion is remembered.
A7. Side, collapsed and ontop settings persist and apply at native startup before showing. No change to full-screen watching or AppBar geometry algorithm. Settings save errors are surfaced. Restored collapsed strip can expand by clicking anywhere.
A8. Legacy todos load unchanged. Storage tests use temporary dirs only, browser tests use mocked IPC with synthetic fixtures, native smoke testing uses isolated appdata. Never insert fixtures into production data.
A9. Native window close (including Alt+F4) must go through the same frontend flush path. An error while natively collapsed temporarily expands the real window (without overwriting the saved collapsed preference), so error/retry controls remain usable. When errors clear, restore the saved native layout. Failed initial reads with no dirty changes may exit normally; failed unsaved writes must not silently exit.

A10. On an external-file conflict, offer an explicit preserve-and-reload action. Sync the current unsaved in-memory snapshot to a new, uniquely named `.unsaved-*` JSON file BEFORE reloading; never overwrite the external primary. If reload fails, keep the archive and expose normal read/recovery controls. If archiving fails, retain the original dirty queue. Settings re-read must also reapply native state; unavailable settings must not prevent temporarily viewing completed tasks (no default-settings write).

Review fixes plan: add a validated JsonStore archive primitive and response envelope for two preserve-and-reload commands; frontend action pauses editing, archives, adopts successfully reloaded state or switches to read-error mode; report archive path visibly. Test all conflicts in synthetic browser fixtures/temp directories and the native QA identity.

## Shared API contract
- Todo: {text:string, done:boolean, created:number, notes:string[]} (notes backward-compatible default [])
- Settings: {side:'left'|'right', collapsed:boolean, ontop:boolean, completed_expanded:boolean}; defaults right/false/true/false.
- load_todos() -> Todo[] or rejected error; save_todos({todos}) -> void or rejected error.
- recover_todos() -> Todo[] or error, explicit action only, preserves original before recovery.
- load_settings() -> Settings or rejected error; save_settings({settings}) -> void or error. Native backend applies new dock/topmost settings only as needed, and persists them. Frontend no longer calls old set_* commands.
- quit_app() -> native exit, frontend calls only after flushing all pending saves.
- set_error_display({visible:boolean}) -> transient native expansion/restoration without modifying persisted settings, serialized with native settings writes.
- Native `sideline-close-requested` event -> frontend flush/quit path; native CloseRequested prevents immediate destruction.

## Plan / work slices
1. Frontend: dist/main.js,index.html,styles.css and tests/frontend.*. Implement scoped note lookup, composition guard, safe drag, serialization/error/retry/quit guards, undo, completed section, settings startup and updates. Use real DOM tests with mocked IPC.
2. Backend: src-tauri/src/lib.rs and new storage.rs (+ Cargo.toml only if needed). Atomic JSON store + tests, mutex serialization, write gating on corrupt data, backup recovery, settings load/save/startup. Leave appbar.rs unchanged except necessary explicit exit cleanup if verified.
3. Integration: cross-check command fields; test corrupted/missing/legacy files, write failures, backup recovery, rapid saving, IME/undo/completed render, and old behavior. Windows cargo test/build, JS checks, real browser rendering, native startup and geometry/settings restore smoke test in isolated data directory.
4. Review spec coverage and risks; fix findings; deploy after checking process and backing up any discovered real data. Keep original backup for rollback and verify final native process/window + no changes to original tasks.

## Rollback
Original source and Desktop exe backed up under backups/20260908-151249. Never restore fixtures over user files. Restore executable only with application stopped. Existing task schema remains compatible.
