# M2 Keybindings

Goal: add, edit, duplicate and delete bindings in any mode through a chord form and a structured
command picker, with conflict detection, while bindings PlaneUI cannot model stay read-only and
byte-identical. The command catalogue built here is reused by window rules in M4.

---

## PUI-030 Keys, modifiers and chords

- **Requirements:** K2, K3
- **Crate:** planeui-core (`keys.rs`)
- **Depends on:** PUI-011
- **Size:** M

### Goal

A precise model of AeroSpace chord notation, including normalisation for conflict checks.

### Scope

- `Modifier::{Cmd, Alt, Ctrl, Shift}`; `KeyName` newtype validated against the AeroSpace 0.21.x
  key list: letters, digits, `keypad0`–`keypad9`, `f1`–`f20`, named specials (`minus`, `equal`,
  `leftSquareBracket`, `sectionSign`, `backtick`, …), keypad specials, arrows, `space`, `tab`,
  `enter`, `esc`, `backspace`, etc. Take the list from AeroSpace's source at the pinned tag and
  link it in a comment.
- `KeyMapping { preset: Preset::{Qwerty, Dvorak, Colemak}, aliases: Vec<(String, KeyName)> }`
  read from `key-mapping.*`; aliases are valid key names.
- `Chord { modifiers: ModifierSet, key: KeyName }` with `parse(&str, &KeyMapping)` returning
  typed errors (unknown key, unknown modifier, duplicate modifier, empty).
- `Chord::normalized()` (canonical modifier order, alias resolved to its key code) for
  comparison; `Chord::format(style)` that preserves the modifier order of the existing raw key
  when editing and uses `cmd-alt-ctrl-shift` order (`alt-shift-x`) for new bindings.

### Acceptance criteria

- [ ] Table-driven tests: valid chords, each error class with message, `shift-alt-h` ==
      `alt-shift-h`, alias equals its target key.
- [ ] proptest: `parse(format(chord)) == chord`.

---

## PUI-031 Command catalogue and binding scripts

- **Requirements:** K4, K5, K6
- **Crate:** planeui-core (`commands.rs`)
- **Depends on:** PUI-011
- **Size:** L

### Goal

A typed model of the supported AeroSpace commands with structured arguments, and a parser that
cleanly separates what we can edit from what must stay read-only.

### Scope

- `Command` enum covering K5: `workspace`, `move-node-to-workspace`,
  `workspace-back-and-forth`, `focus`, `move`, `resize`, `layout`, `fullscreen`, `join-with`,
  `flatten-workspace-tree`, `balance-sizes`, `close`, `close-all-windows-but-current`,
  `focus-monitor`, `move-node-to-monitor`, `move-workspace-to-monitor`, `mode`,
  `reload-config`, `exec-and-forget`. Arguments as typed fields: `Direction`, `WorkspaceTarget`
  (name, `next`/`prev` with flags), `ModeName`, signed amount for `resize`, layout option lists,
  monitor targets, a single shell-command string for `exec-and-forget`.
- Only the flags documented for 0.21.x per command; anything else (unknown flag, unknown
  command) is `Unsupported`.
- `Script::parse(&Item) -> ParsedScript` where the item is a string or an array of strings:
  - All elements parse into catalogue commands → `Editable(Vec<Command>)` plus the original
    shape (string vs array).
  - Any `;`, `&&`, `||`, `|`, parentheses (outside the `exec-and-forget` argument), unknown
    commands or flags → `ReadOnly { reason }`.
- `Command::format()` producing the exact canonical text AeroSpace accepts; quoting rules for
  `exec-and-forget` and workspace names with spaces.
- `CATALOGUE_VERSION` constant (e.g. `0.21.3`) for the compatibility warning (PUI-058).
- Command metadata for the UI: display name, short help, argument specs.

### Acceptance criteria

- [ ] Every binding in `aerospace-default.toml` and `sander.toml` parses to `Editable` or to
      `ReadOnly` with a reason; the test prints the read-only ones so Sander can sanity-check.
- [ ] proptest: `parse(format(commands)) == commands` for generated editable command lists.
- [ ] Table-driven tests for read-only detection: each shell operator, unknown command, unknown
      flag.

---

## PUI-032 Bindings in the ConfigView

- **Requirements:** K1, K6
- **Crate:** planeui-core (`config_view.rs`)
- **Depends on:** PUI-030, PUI-031
- **Size:** M

### Goal

Read every mode and binding in file order with enough detail to edit or display read-only.

### Scope

- `modes: Vec<Mode { name: ModeName, bindings: Vec<Binding> }>`, `main` first, then file order.
- `Binding { raw_key, chord: Result<Chord, ChordError>, script: ParsedScript, inline_comment:
  Option<String>, span }`. Inline comment is the trailing `# …` from the item's decor.
- Handles `[mode.main.binding]` tables, dotted `mode.main.binding.alt-h = …`, and inline tables.
  A shape we cannot edit safely marks the whole mode's bindings read-only rather than guessing.

### Acceptance criteria

- [ ] insta snapshot of the bindings view for `aerospace-default.toml`, `sander.toml` and
      `dotted-keys.toml`.

---

## PUI-033 Binding edit operations

- **Requirements:** K1, F3
- **Crate:** planeui-core
- **Depends on:** PUI-032
- **Size:** M

### Goal

Add, change, duplicate and remove bindings touching only their own line.

### Scope

- `upsert_binding(doc, mode, original_raw_key: Option<&str>, chord, commands)`: editing in place
  keeps the key's position, its inline comment and the string-vs-array shape (a single command
  stays a string unless more are added). Changing the chord renames the key in place (same
  position, same comment). New bindings append after the mode's last binding.
- `remove_binding(doc, mode, raw_key)`.
- `duplicate_binding(doc, from_mode, raw_key, to_mode)`: copies the raw value item verbatim,
  creating `[mode.<to>.binding]` in the file's style if missing.
- Refuses (`EditError::ReadOnly`) to rewrite a binding whose script is `ReadOnly`; delete and
  duplicate of read-only bindings are allowed because they copy or drop bytes verbatim.

### Acceptance criteria

- [ ] insta diff snapshots: each op on table, dotted and inline styles; diffs touch only the
      binding's line(s).
- [ ] Renaming the chord of a commented binding keeps the comment.

---

## PUI-034 Binding validation

- **Requirements:** K3, F7
- **Crate:** planeui-core (`validate.rs`)
- **Depends on:** PUI-032
- **Size:** S

### Goal

Errors for bindings that AeroSpace would reject or that silently override each other.

### Scope

- Errors: invalid chord, duplicate normalised chord within a mode (message names the existing
  binding: "alt-h is already bound to `focus left` in main"), `mode` command referring to an
  unknown mode, missing `main` mode, invalid command arguments.
- A `check_chord_available(view, mode, chord, ignoring_raw_key)` helper for instant feedback in
  the form before commit.
- Warning (lint): chord shadows a common macOS shortcut (cmd-tab, cmd-space, cmd-h, cmd-q).

### Acceptance criteria

- [ ] Table-driven tests per error class with expected messages, including alias-vs-key and
      reordered-modifier conflicts.

---

## PUI-035 Keybindings section: mode tabs and table

- **Requirements:** K1, K6
- **Crate:** planeui-app
- **Depends on:** PUI-033, PUI-034, PUI-020
- **Size:** M

### Goal

Browse bindings per mode and perform the row actions that need no form.

### Scope

- Mode tabs, `main` first. Table columns: chord (kbd chips), commands (formatted), inline
  comment, row actions.
- Delete commits immediately (undoable). "Duplicate to mode…" opens a mode picker; a conflict in
  the target mode shows the error and blocks.
- Read-only rows are visually distinct and labeled "edit in a text editor" with the reason.
- Empty state for modes with no bindings.

### Acceptance criteria

- [ ] `#[gpui::test]`: delete removes exactly that line on disk; duplicate into a mode with the
      same chord is blocked with the conflict message.

---

## PUI-036 Binding form and command picker

- **Requirements:** K2, K3, K4
- **Crate:** planeui-app
- **Depends on:** PUI-035
- **Size:** L

### Goal

Create and edit bindings without typing chord names or command syntax.

### Scope

- "Add binding" opens a form row at the top of the table; "Edit" opens it in place.
- Modifier checkboxes (cmd, alt, ctrl, shift) and a searchable key dropdown listing every key
  name plus `key-mapping` aliases.
- Conflict check runs on every chord change and shows the inline error immediately.
- Command picker: searchable list from the catalogue metadata; per-command argument fields
  (direction dropdown, workspace name with suggestions from existing bindings and
  `persistent-workspaces`, mode dropdown, signed number, layout option chips, monitor target,
  text for `exec-and-forget`). Multiple commands as an ordered list with add, remove, reorder.
- Commit rule: the row commits when complete and valid (the form's own commit, not per field);
  Escape cancels; Enter commits. An incomplete row is never written.

### Acceptance criteria

- [ ] `#[gpui::test]`: building `alt-shift-p` → `workspace P` adds one line in `alt-shift-x`
      order; editing an existing `shift-alt-…` binding keeps its order.
- [ ] Entering a taken chord blocks the commit and names the existing binding.
- [ ] The full flow is keyboard-only operable.

---

## PUI-037 Manage modes

- **Requirements:** K7 (Should)
- **Crate:** planeui-core + planeui-app
- **Depends on:** PUI-036
- **Size:** M

### Goal

Create and delete modes safely.

### Scope

- Core: `add_mode(doc, name)` (creates `[mode.<name>.binding]` in file style),
  `remove_mode(doc, name)` returning `Blocked { referencing: Vec<(mode, raw_key)> }` when any
  binding switches to it; `main` cannot be deleted.
- Lint warning: non-main mode with no binding that runs `mode main`.
- UI: "+" tab to add a mode (name validated), delete from the tab's context menu, warning badge
  on the tab.

### Acceptance criteria

- [ ] Table-driven tests for blocked deletion and the no-way-back warning.
- [ ] Removing an unreferenced mode removes only its table(s).
