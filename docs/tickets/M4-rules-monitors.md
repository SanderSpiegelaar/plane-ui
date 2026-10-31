# M4 Rules, monitors and per-monitor gaps

Goal: window rules, workspace-to-monitor assignment and per-monitor gaps, all driven by live data
from AeroSpace, with manual entry as a fallback when AeroSpace is not running.

---

## PUI-050 Live data from AeroSpace

- **Requirements:** W2, M2, F11
- **Crate:** planeui-core (`aerospace.rs`) + planeui-app
- **Depends on:** PUI-014
- **Size:** M

### Scope

- Extend the `Aerospace` trait: `list_apps() -> Vec<App { bundle_id, name }>`,
  `list_monitors() -> Vec<Monitor { id, name, is_main }>`,
  `list_workspaces() -> Vec<WorkspaceName>` using the `--json` flags; serde models tolerate
  unknown fields.
- Recorded JSON fixtures from Sander's machine in `tests/fixtures/cli/`, including a
  multi-monitor capture.
- App side: `LiveData` entity fetched in the background on startup and on window activation (at
  most once per few seconds); `Unavailable` state when AeroSpace is not running so pickers switch
  to manual entry.

### Acceptance criteria

- [ ] Parser tests against the recorded fixtures; unknown extra fields ignored.
- [ ] `#[gpui::test]`: with a not-running fake, pickers render manual entry only.

---

## PUI-051 Window rules in the ConfigView

- **Requirements:** W1, W4
- **Crate:** planeui-core
- **Depends on:** PUI-031
- **Size:** M

### Goal

Read `on-window-detected` in file order and classify each rule as editable or read-only.

### Scope

- Detect the container shape: `[[on-window-detected]]` array of tables or
  `on-window-detected = [ … ]` inline array (TOML 1.1 multi-line inline tables). Mixing both is
  already a validation error (PUI-012).
- `WindowRule { matcher: Matcher, run: ParsedScript, check_further_callbacks: Option<bool>,
  span }`.
- `Matcher::Conditions(Vec<Condition>)` for the new syntax when the `if` string is a
  conjunction we model: app bundle id equals, app name matches regex, window title matches
  regex, workspace equals. Verify the exact `test` interpolation variable names and operators
  (`=`, `~=`, `test-not`, how conjunctions are written) against the AeroSpace 0.21.x guide
  before coding, and link the source.
- `Matcher::Legacy(LegacyMatcher)` for `if.app-id`, `if.app-name-regex-substring`,
  `if.window-title-regex-substring`, `if.workspace`, `if.during-aerospace-startup`.
- `Matcher::ReadOnly { reason }` for anything else (complex expressions, `||`, `eval`, unknown
  keys).
- Regex conditions validated with `regex` as warnings only (roadmap D3).

### Acceptance criteria

- [ ] insta snapshot of the parsed rules for each rules fixture; every rule classified.
- [ ] Rule without `if` is a validation error (0.21 behaviour).

---

## PUI-052 Window rule edit operations

- **Requirements:** W1, W3, W4, F3
- **Crate:** planeui-core
- **Depends on:** PUI-051
- **Size:** M

### Scope

- `add_rule`, `update_rule`, `remove_rule`, `move_rule(from, to)` in the file's container shape.
  An empty file gets `[[on-window-detected]]` (array of tables).
- New rules are written as `if = 'test %{app-bundle-id} = …'` (exact form per PUI-051's
  verification).
- Editing a legacy rule keeps the legacy keys (`if.app-id = …`); no conversion.
- Read-only rules can be moved and deleted (bytes moved verbatim), never rewritten.
- `run` uses the PUI-031 script formatter and keeps string-vs-array shape.
- Moving keeps each rule's comments attached to it.

### Acceptance criteria

- [ ] insta diffs for each op on both container shapes and on legacy rules.
- [ ] proptest: random move sequences preserve the multiset of rule byte blocks.

---

## PUI-053 Window rules UI

- **Requirements:** W1, W2, W3
- **Crate:** planeui-app
- **Depends on:** PUI-052, PUI-050, PUI-036
- **Size:** L

### Scope

- Ordered cards: "When <conditions> then <actions>" plus "Keep checking further rules" switch.
  Read-only cards show their raw text and reason.
- "New rule" appends a card in edit state. Condition "App is" uses a searchable list from
  `list-apps` (name and bundle id) with manual bundle id entry; plus app name matches, window
  title matches, workspace is.
- Actions reuse the PUI-036 command picker.
- Reorder by drag and drop, plus "Move up"/"Move down" actions for keyboard use.
- A card commits when complete and valid, like the binding form.

### Acceptance criteria

- [ ] `#[gpui::test]`: create rule from a fake app list writes the expected `if` and `run`;
      moving a card writes one move; editing a legacy rule keeps legacy syntax.

---

## PUI-054 Monitor patterns and workspace-to-monitor core

- **Requirements:** M1, M2, M3 (core)
- **Crate:** planeui-core
- **Depends on:** PUI-011
- **Size:** M

### Scope

- `MonitorPattern::{Main, Secondary, Index(NonZeroU32), Regex(String)}` with parse and format
  matching AeroSpace's grammar (shared with per-monitor gaps).
- View: `workspace_to_monitor: Vec<Assignment { workspace, patterns: Vec<MonitorPattern>,
  shape: Single | List }>` in file order.
- Edits: add/remove assignment; add/remove/reorder/change patterns; a single pattern stays a
  string until a second is added.
- `resolve(patterns, monitors) -> Option<&Monitor>` mirroring AeroSpace's first-match semantics
  (case-insensitive substring regex) for the preview.
- Regex validation as a warning (roadmap D3).

### Acceptance criteria

- [ ] Table-driven pattern parse/resolve tests against the recorded monitor fixture.
- [ ] insta diffs for each edit.

---

## PUI-055 Workspaces and monitors UI

- **Requirements:** M1, M2, M3
- **Crate:** planeui-app
- **Depends on:** PUI-054, PUI-050
- **Size:** M

### Scope

- Table rows: workspace name, ordered pattern chips (add, edit, delete, reorder).
- Pattern picker offers `main`, `secondary`, each connected monitor's index and name; free regex
  entry with inline validation.
- Each row shows the resolved monitor or "no match" (live), hidden when AeroSpace is not
  running.

### Acceptance criteria

- [ ] `#[gpui::test]`: picking a monitor name writes the expected pattern; preview updates when
      patterns change.

---

## PUI-056 Per-monitor gaps core

- **Requirements:** G2 (Should)
- **Crate:** planeui-core
- **Depends on:** PUI-054, PUI-040
- **Size:** M

### Scope

- Parse `[{ monitor.main = 16 }, { monitor."dell" = 32 }, 24]` into
  `PerMonitor { rules: Vec<(MonitorPattern, u32)>, default: u32 }`; anything else stays
  read-only.
- Edits: switch a field const → per-monitor (current value becomes the default), per-monitor →
  const (default becomes the value), add/remove/reorder/change rule rows, change default.
- Validation: default present and last; values ≥ 0.

### Acceptance criteria

- [ ] insta diffs for each edit; existing per-monitor arrays round-trip unchanged when not
      edited.

---

## PUI-057 Per-monitor gaps UI

- **Requirements:** G2
- **Crate:** planeui-app
- **Depends on:** PUI-056, PUI-041, PUI-050
- **Size:** S

### Scope

- Per gap field, a "Per monitor" toggle revealing pattern + value rows (pattern picker from
  PUI-055) and a default value. Replaces the read-only display from PUI-041.

### Acceptance criteria

- [ ] `#[gpui::test]`: switching to per-monitor and back restores the original line via undo.

---

## PUI-058 AeroSpace version compatibility warning

- **Requirements:** NFR Compatibility
- **Crate:** planeui-core + planeui-app
- **Depends on:** PUI-031, PUI-014
- **Size:** S

### Scope

- Compare the installed version with `CATALOGUE_VERSION`; newer minor/major shows a status bar
  warning ("AeroSpace 0.22 is newer than PlaneUI's command catalogue (0.21): unknown commands
  stay read-only").

### Acceptance criteria

- [ ] Version comparison table tests, including `-Beta` suffixes.
