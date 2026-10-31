# M1 Apply engine

Goal: the full change pipeline (locate → edit → validate → external check → snapshot → atomic
write → reload → status), undo/redo and snapshots, proven with tests and wired into a minimal app
shell. No section editors yet, except one debug control used to exercise the pipeline.
Core tickets (010–018) can mostly run in parallel with each other once PUI-010 lands.

---

## PUI-010 ConfigDoc edit primitives

- **Requirements:** F2, F3
- **Crate:** planeui-core (`config_doc.rs`)
- **Depends on:** PUI-003
- **Size:** L

### Goal

The small set of document mutations every section editor is built from, each changing only the
targeted key or array element and keeping the file's existing style.

### Scope

- `KeyPath` newtype (`["gaps", "outer", "left"]`), with `Display` as dotted TOML path.
- `ConfigDoc::get(&KeyPath) -> Option<&Item>` that resolves a path regardless of how it is
  written: `[gaps.outer]` table, `[gaps]` with dotted `outer.left`, root dotted
  `gaps.outer.left`, or inline table.
- `ConfigDoc::set_value(&KeyPath, Value) -> Result<(), EditError>`:
  - Existing key: replace the value only, keeping the key's decor (leading comment, trailing
    inline comment, spacing) and the value's own decor.
  - New key: insert next to its siblings in the style they use (dotted vs table, ordering after
    the last sibling). With no siblings, follow the nearest-parent style; at root, insert above
    the first `[table]` header, never after it (the "root key absorbed by a table" pitfall).
- `ConfigDoc::remove(&KeyPath) -> Result<(), EditError>`: removes the key and its own
  leading-comment decor only; drops empty implicit parent tables it created, never explicit
  `[table]` headers the user wrote.
- Array-element helpers used later by bindings and rules: `array_insert`, `array_remove`,
  `array_move`, `array_set` for both inline arrays and arrays of tables.
- `EditError` (thiserror) with the key path, e.g. "gaps.outer is not a table".

### Acceptance criteria

- [ ] Every primitive leaves all bytes outside the touched key identical.
- [ ] Setting a key that exists in each of the four styles keeps that style.
- [ ] Inserting a new root key in a file whose last section is a table puts it before the first
      table header, and the result re-parses with the key at root.
- [ ] Remove then re-add of the same key in a commented file does not disturb unrelated comments.

### Tests

- insta snapshots of the unified diff (`similar`) for each primitive × each style fixture.
- proptest: random sequences of `set_value`/`remove` on known keys over the fixtures; assert the
  result re-parses, every untouched top-level table is byte-identical to the original, and
  replaying the inverse sequence restores the original bytes.

---

## PUI-011 ConfigView skeleton

- **Requirements:** A4 (data), groundwork for S1, G1, K1, W1, M1
- **Crate:** planeui-core (`config_view.rs`)
- **Depends on:** PUI-010
- **Size:** S

### Goal

A typed, read-only view derived from the document after every edit, with the structure later
milestones fill in.

### Scope

- `ConfigView::from_doc(&ConfigDoc) -> ConfigView`; never fails as a whole. A bad value becomes
  `Field::Invalid { span, raw }` instead of aborting the load.
- `Located<T> { value: T, span: Option<Range<usize>> }` for diagnostics.
- Section stubs filled in by later tickets: `general` (PUI-042), `gaps` (PUI-040), `modes`
  (PUI-032), `window_rules` (PUI-051), `workspace_to_monitor` (PUI-054).
- `out_of_scope: Vec<Located<KeyPath>>`: keys PlaneUI does not edit (`after-startup-command`,
  `on-focus-changed`, `on-focused-monitor-changed`, `on-mode-changed`,
  `exec-on-workspace-change`, `exec.*`, `key-mapping.key-notation-to-key-code`, deprecated keys,
  unknown keys). Feeds A4.

### Acceptance criteria

- [ ] View of `callbacks-and-exec.toml` lists every callback/exec key in `out_of_scope`.
- [ ] A fixture with a wrong-typed value builds a view with that field marked invalid.

---

## PUI-012 Validation framework

- **Requirements:** F7
- **Crate:** planeui-core (`validate.rs`)
- **Depends on:** PUI-011
- **Size:** S

### Goal

One entry point that decides whether a document may be written, extensible per section.

### Scope

- `ValidationIssue { key_path, span, severity: Severity::{Error, Warning}, kind, message }` and
  `validate(&ConfigDoc, &ConfigView) -> Vec<ValidationIssue>`.
- L0: TOML syntax (parse errors from PUI-003, with line/column).
- L1 schema: known top-level keys and types for the keys PlaneUI understands (list from
  SPEC "Config surface"); unknown keys are **warnings**, never block writes (they may be valid in
  a newer AeroSpace and the file already has them). Mixing `[[on-window-detected]]` and
  `on-window-detected = [...]` is an error (Discussion #2146).
- A `Rule` trait or plain function list so M2–M4 add L2 checks (chords, commands, patterns,
  regex) without touching the pipeline.
- `ValidationReport::blocks_write()` = any `Error`.

### Acceptance criteria

- [ ] Table-driven tests: one row per error class with expected message text.
- [ ] Every fixture except those deliberately invalid validates with zero errors.

---

## PUI-013 Config path resolution

- **Requirements:** F1, F10, invariant 7
- **Crate:** planeui-core (`store.rs`)
- **Depends on:** PUI-001
- **Size:** S

### Goal

Find the file AeroSpace actually uses, mirroring its lookup rules.

### Scope

- `resolve_config_path(env: &PathEnv, server_path: Option<PathBuf>) -> Result<ResolvedConfig,
  LocateError>` where `PathEnv` carries `HOME` and `XDG_CONFIG_HOME` so tests inject them.
- Order: the running server's path (`aerospace config --config-path`, from PUI-014) when
  available; else `~/.aerospace.toml` and `${XDG_CONFIG_HOME:-~/.config}/aerospace/aerospace.toml`.
- Both exist → `LocateError::Ambiguous { home, xdg }` (explain AeroSpace rejects this).
  Neither → `LocateError::NotFound { suggested: PathBuf }` (the XDG path).
- `ResolvedConfig { display_path, target_path }`: `target_path` is the symlink-resolved file
  that writes go to; `display_path` is what the user sees.

### Acceptance criteria

- [ ] tempfile tests for: home only, XDG only, XDG via env var, both (ambiguous), none,
      symlink to a dotfiles dir (target resolved, link kept), server path overriding both.

---

## PUI-014 `Aerospace` trait and CLI implementation

- **Requirements:** F1, F8, F11, NFR Robustness
- **Crate:** planeui-core (`aerospace.rs`)
- **Depends on:** PUI-001
- **Size:** M

### Goal

All talk to AeroSpace goes through one trait with a real CLI impl and a fake for tests.

### Scope

- `trait Aerospace { fn version(&self); fn config_path(&self); fn reload_config(&self); }`
  (M4 adds `list_apps`, `list_monitors`, `list_workspaces`). Synchronous; the app calls it from
  `background_spawn`.
- `CliAerospace`: find the binary on `PATH`, then `/opt/homebrew/bin/aerospace`, then
  `/usr/local/bin/aerospace` (GUI apps do not inherit the shell PATH). Spawn with
  `std::process::Command`, poll `try_wait` with a 3 s deadline, kill on timeout.
- `ReloadOutcome::{Applied, Failed { errors: Vec<ReloadMessage>, raw: String }, NotRunning}`;
  parse `reload-config --no-gui` stdout/stderr and exit code. `ReloadMessage` keeps any
  `path: message` prefix so the UI can name the key.
- `AerospaceVersion` parsed from `aerospace --version` (e.g. `0.21.3-Beta`).
- `ReloadError` / `CliError`: `BinaryNotFound`, `Timeout`, `Io`, `UnexpectedOutput`. Not running
  is an outcome, not an error.
- `FakeAerospace` (behind `#[cfg(any(test, feature = "test-support"))]`) with scripted results.

### Acceptance criteria

- [ ] Parsers tested against recorded output in `tests/fixtures/cli/` (success, errors, server
      not running, unknown version string). Record these once from Sander's machine.
- [ ] Timeout test uses a fake binary script (`sleep 5`) in a tempdir; returns `Timeout` within
      ~3 s. No test invokes the real `aerospace`.

---

## PUI-015 Store: atomic write and external-change detection

- **Requirements:** F9, F10, invariants 4 and 5
- **Crate:** planeui-core (`store.rs`)
- **Depends on:** PUI-013
- **Size:** M

### Goal

Load and write the config so a write is all-or-nothing and never clobbers an edit made outside
PlaneUI.

### Scope

- `ConfigStore::load(&ResolvedConfig) -> Result<Loaded, LoadError>` returning bytes and a
  `DiskStamp` (the exact bytes, or length + a hash; content comparison, not mtime).
- `ConfigStore::check_unchanged(&DiskStamp) -> Result<(), ExternalChange>`.
- `ConfigStore::write_atomic(target, bytes)`: temp file in the same directory, `sync_all`,
  `rename` over the target, keep original permissions; write through symlinks to the target.
- Preserve the file's line endings: the document already holds them, so this is about not
  normalising anywhere in the write path.

### Acceptance criteria

- [ ] tempfile tests: atomic write replaces content; symlink stays a symlink and the target
      changes; external edit between load and write is detected; temp file is cleaned up when
      the rename fails (e.g. read-only directory).

---

## PUI-016 Snapshots with rotation

- **Requirements:** U3, U4 (core), invariant 3
- **Crate:** planeui-core (`store.rs`)
- **Depends on:** PUI-015
- **Size:** S

### Goal

A byte-for-byte copy of the current file before every write, newest 10 kept.

### Scope

- `SnapshotStore::new(dir)`; default dir `dirs::data_dir()/PlaneUI/snapshots` (= Application
  Support on macOS). Created on first use.
- `take(&current_bytes) -> Result<SnapshotId, SnapshotError>` names files with a sortable UTC
  timestamp (roadmap D1), with a suffix on same-millisecond collisions; then deletes the oldest
  beyond 10.
- `list() -> Vec<SnapshotMeta { id, taken_at, size }>` newest first; `read(id) -> bytes`.

### Acceptance criteria

- [ ] 11th snapshot deletes the oldest; only files matching our naming scheme are counted or
      deleted (stray files in the dir are left alone).
- [ ] Snapshot failure (read-only dir) returns an error and is tested to block the write in
      PUI-018.

---

## PUI-017 Undo/redo history

- **Requirements:** U1, U2, invariant 8
- **Crate:** planeui-core
- **Depends on:** PUI-010
- **Size:** S

### Goal

Exact, unlimited in-session undo by storing whole documents (configs are small).

### Scope

- `History { undo: Vec<Entry>, redo: Vec<Entry> }` with `Entry { before: String, after: String,
  label: String }`. `record`, `undo() -> Option<String>`, `redo() -> Option<String>`; a new
  record clears redo.
- `label` is a short plain-language description ("Set accordion-padding to 20") so U5 can use it
  later.
- External reload (PUI-018) does not clear history but marks entries before it as crossing an
  external change; undoing across it must check the disk first like any other write.

### Acceptance criteria

- [ ] proptest: any sequence of records followed by the same number of undos returns the
      original bytes exactly; redo after undo returns the later bytes exactly.

---

## PUI-018 Apply pipeline

- **Requirements:** F4, F7, F9, U3, invariants 1–6
- **Crate:** planeui-core
- **Depends on:** PUI-012, PUI-014, PUI-016, PUI-017
- **Size:** M

### Goal

One synchronous, testable function that turns "here is the new document" into a written,
reloaded config — or a reason it did not happen.

### Scope

- `apply(ctx: &ApplyContext, stamp: &DiskStamp, new_text: &str) -> ApplyOutcome` where
  `ApplyContext` holds the store, snapshot store and `&dyn Aerospace`.
- Steps, aborting on the first failure: re-parse and `validate` (errors → `Rejected(issues)`);
  `check_unchanged` (→ `ExternalChange { disk_bytes }`); `take` snapshot (→ `SnapshotFailed`,
  no write); `write_atomic`; `reload_config` (→ `Written { reload: ReloadOutcome, new_stamp }`).
- Logs each step with timings (`log::debug!`) for the NFR latency check in PUI-065.

### Acceptance criteria

- [ ] Tests with `FakeAerospace` + tempdirs cover: success; validation error leaves disk
      untouched and no snapshot taken; external change leaves disk untouched; snapshot failure
      leaves disk untouched; reload failure still reports `Written` with the errors; AeroSpace
      not running reports `Written` + `NotRunning`.

---

## PUI-019 ConfigModel entity and serialized write queue

- **Requirements:** F4, F6, F8, F9, F11, U1
- **Crate:** planeui-app
- **Depends on:** PUI-002, PUI-018
- **Size:** L

### Goal

The single `ConfigModel` entity that owns the document, history, validation and save status, and
runs the pipeline off the UI thread one write at a time.

### Scope

- `ConfigModel { doc: ConfigDoc, view: ConfigView, issues, history, stamp, status: SaveStatus,
  in_flight: Option<Task<()>>, pending: Option<PendingChange> }`.
- `commit(new_doc, label, cx)`: update in-memory state, `cx.notify()`, and enqueue. If a write is
  in flight, replace `pending` with the latest state (coalescing); when the in-flight task
  finishes, start the pending one. Exactly one write + one reload per burst.
- Run `apply` in `cx.background_spawn`, apply the outcome with `this.update(cx, ..)` from
  `cx.spawn(async move |this, cx| ..)`.
- `ExternalChange` outcome: replace the doc with disk content, rebuild the view, set a
  "Reloaded from disk" toast event, drop the pending change (its base is stale; it shows as an
  inline note rather than being reapplied).
- `undo` / `redo` actions (`actions!(planeui, [Undo, Redo])`, `cmd-z`, `cmd-shift-z`) that push
  the history state through `commit`.
- `SaveStatus::{Idle, Writing, Applied { at }, ReloadFailed { messages }, NotRunning, Error(..)}`
  emitted as events for the status bar and banners.

### Acceptance criteria

- [ ] `#[gpui::test]`: three commits while a write is in flight produce exactly two writes (the
      in-flight one and one trailing with the last state).
- [ ] Undo after a commit restores the previous bytes on disk and reloads (fake).
- [ ] External change during a burst surfaces the toast event and never overwrites disk.
- [ ] No `std::fs` or process calls on the UI thread (review checklist item in the PR).

---

## PUI-020 Commit-rule inputs

- **Requirements:** F5, F7
- **Crate:** planeui-app
- **Depends on:** PUI-019
- **Size:** M

### Goal

Reusable field components that implement the commit rules once, so section editors cannot get
them wrong.

### Scope

- `CommitTextField` / `CommitNumberField` wrapping gpui-kit inputs: commit on Enter, on blur, or
  700 ms after the last keystroke (debounce task stored in a field; replacing it cancels the
  previous one; GPUI executor timer). Commit only when the local validator passes; otherwise
  show the inline error and keep the invalid text in the field.
- Stepper (number +/−) commits 400 ms after the last click.
- Toggles, selects and pickers commit immediately (thin helpers so call sites stay uniform).
- Fields expose `has_error` so sections can show a badge (A2, PUI-061).
- A debug control (`accordion-padding` number field) in the placeholder view to exercise the
  pipeline end to end; removed or replaced in PUI-043.

### Acceptance criteria

- [ ] `#[gpui::test]` with a fake clock: typing five characters within 700 ms commits once;
      Enter commits immediately; blur commits; invalid text never commits and the error stays
      after blur.
- [ ] Four stepper clicks within 400 ms commit once.

---

## PUI-021 App shell and status bar

- **Requirements:** A2, A3, F8, F11
- **Crate:** planeui-app
- **Depends on:** PUI-019
- **Size:** M

### Goal

The permanent window layout: sidebar navigation, content pane, status bar, banner and toast
areas.

### Scope

- Sidebar (gpui-kit sidebar): Keybindings, Gaps, General, Window rules, Workspaces and monitors,
  Snapshots. Sections are placeholders until their milestone; keyboard navigable.
- Status bar: config display path, AeroSpace version, running state, last apply result
  ("Applied 14:03:12", "AeroSpace not running: changes apply on next start").
- Reload error banner with AeroSpace's error text and an "Undo last change" action (F8).
- Toast area using gpui-kit notifications ("Reloaded from disk").
- On window activation, check the disk stamp in the background and reload the view if it
  changed (F9 beyond "before the next write").

### Acceptance criteria

- [ ] `#[gpui::test]`: a failed reload shows the banner; its Undo runs the undo action.
- [ ] Status bar reflects `Applied`, `NotRunning` and `ReloadFailed` states.

---

## PUI-022 Startup flows

- **Requirements:** F1
- **Crate:** planeui-app (+ core helper)
- **Depends on:** PUI-021
- **Size:** S

### Goal

Handle every outcome of locating the config before the main UI shows.

### Scope

- Ambiguous: a screen naming both files, explaining AeroSpace refuses to start with both, and
  that PlaneUI edits nothing until one is removed or renamed (no automatic archiving). A
  "Check again" button re-runs resolution.
- Not found: offer "Create from AeroSpace's default config" (writes the vendored
  `aerospace-default.toml` to the suggested path atomically, then opens it); no snapshot needed
  since there is no previous file.
- AeroSpace binary not found: open the config anyway, status bar says so, reload is skipped.
- Startup work (resolve, load, version, validation) happens in the background; the window shows a
  loading state, not a blank frame.

### Acceptance criteria

- [ ] Each flow is reachable with fakes in a `#[gpui::test]`.
- [ ] Creating from default produces a file byte-identical to the vendored fixture.

---

## PUI-023 auto-reload-config banner

- **Requirements:** F12, invariant 9
- **Crate:** planeui-app (+ core prefs)
- **Depends on:** PUI-019, PUI-021
- **Size:** S

### Goal

Offer, never force, turning off `auto-reload-config`.

### Scope

- Core `Prefs` stored as JSON (`serde_json`) in `dirs::data_dir()/PlaneUI/prefs.json`:
  `dismissed_auto_reload: Vec<PathBuf>` keyed by config path. Written atomically, off the UI
  thread.
- Banner when the opened config has `auto-reload-config = true` and the path is not dismissed.
  "Turn off" commits `auto-reload-config = false` through `ConfigModel::commit` (snapshot, write,
  reload, undoable). "Keep on" adds the path to prefs.
- If the user later sets it to true in General (PUI-043) the banner is not shown again for that
  path (record the dismissal on that action too).

### Acceptance criteria

- [ ] `#[gpui::test]`: Turn off → file has `false` with surrounding bytes unchanged; Cmd-Z
      restores `true`. Keep on → banner gone and stays gone after relaunch (prefs reloaded).

---

## PUI-024 Snapshots screen

- **Requirements:** U4
- **Crate:** planeui-app (+ core diff helper)
- **Depends on:** PUI-016, PUI-019, PUI-021
- **Size:** M

### Goal

List the 10 snapshots and restore one through the normal pipeline.

### Scope

- Core `line_diff_stats(old, new) -> (added, removed)` using `similar` (roadmap D2).
- Screen lists snapshots newest first: local date and time, `+added / −removed` against the
  current file. Listing and diffing run in the background.
- Restore: read the chosen snapshot first, then `commit` its content as a normal change (the
  pipeline snapshots the current file, rotation applies, writes, reloads). Label
  "Restored snapshot from <time>"; Cmd-Z undoes it.
- Keyboard: arrow keys move the selection, Enter restores after a confirmation.

### Acceptance criteria

- [ ] `#[gpui::test]`: restoring snapshot N writes its bytes exactly, creates a new snapshot of
      the pre-restore file, and undo returns to the pre-restore bytes.
- [ ] Snapshot read failure shows an error and writes nothing.
