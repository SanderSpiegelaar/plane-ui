# M3 Gaps and General

Goal: constant gaps, the general settings form, config-version and persistent workspaces. The end
of M3 is the **first usable build**: Sander starts using PlaneUI for real edits.

---

## PUI-040 Gaps core

- **Requirements:** G1, F3
- **Crate:** planeui-core
- **Depends on:** PUI-011
- **Size:** M

### Goal

Read all six gap values in any style and edit constant ones without touching per-monitor arrays.

### Scope

- `Gaps { inner_horizontal, inner_vertical, outer_left, outer_right, outer_top, outer_bottom }`,
  each `GapValue::{Absent, Const(u32), PerMonitor(PerMonitorRaw), Invalid}`. `PerMonitor` is
  parsed fully in PUI-056; here it is only detected and marked read-only.
- `set_gap(doc, GapField, u32)` and `reset_gap(doc, GapField)` via PUI-010 primitives, styles:
  `[gaps]` + dotted, `[gaps.inner]`/`[gaps.outer]`, root dotted, inline tables.
- `set_outer_linked(doc, u32)`: one edit setting all four outer values (one undo entry).
- Validation: integers ≥ 0; refuses to write a per-monitor field.

### Acceptance criteria

- [ ] insta diff snapshots per style; setting one gap changes one line.
- [ ] A per-monitor field is never rewritten by any gaps op (test asserts its bytes).

---

## PUI-041 Gaps UI

- **Requirements:** G1
- **Crate:** planeui-app
- **Depends on:** PUI-040, PUI-020
- **Size:** S

### Scope

- Six `CommitNumberField`s with steppers grouped as Inner and Outer, effective default shown
  when absent ("default: 0"), "Reset to default" per field.
- "Link all outer" toggle: when on, editing one outer field sets all four in a single commit.
  Turning it on when values differ asks which value to apply.
- Per-monitor fields show their raw value read-only with "per-monitor (editable in a later
  version)" until PUI-057.

### Acceptance criteria

- [ ] `#[gpui::test]`: typing 12 in outer-left with link on writes four lines in one write.

---

## PUI-042 General settings core

- **Requirements:** S1, S2
- **Crate:** planeui-core
- **Depends on:** PUI-011
- **Size:** M

### Goal

Typed access, defaults and set/reset for the S1 scalar keys.

### Scope

- A table of settings: key path, type, default (from the SPEC "Config surface", checked against
  the vendored default config), allowed values:
  `start-at-login` (bool, false), `auto-reload-config` (bool, false), `accordion-padding`
  (int ≥ 0, 30), `default-root-container-layout` (tiles | accordion, tiles),
  `default-root-container-orientation` (horizontal | vertical | auto, auto),
  `enable-normalization-flatten-containers` (bool, true),
  `enable-normalization-opposite-orientation-for-nested-containers` (bool, true),
  `automatically-unhide-macos-hidden-apps` (bool, false), `focus-follows-mouse.enabled` (bool,
  false), `key-mapping.preset` (qwerty | dvorak | colemak, qwerty).
- Enums are Rust enums, not strings.
- `set_setting(doc, Setting, value)` adds or changes the key; `reset_setting` removes it.
- L1 validation of these keys' types and enum values.

### Acceptance criteria

- [ ] insta diffs: set on absent key inserts at root above the first table; reset removes only
      that line.
- [ ] Table-driven validation tests for wrong types and unknown enum values.

---

## PUI-043 General settings UI

- **Requirements:** S1, S2, F12 (interaction)
- **Crate:** planeui-app
- **Depends on:** PUI-042, PUI-020
- **Size:** S

### Scope

- Switches, selects and a number field per setting; each shows "default: …" when absent and a
  "Reset to default" action when present.
- Turning `auto-reload-config` on records the F12 dismissal for this path (PUI-023).
- Replace the M1 debug control.

### Acceptance criteria

- [ ] `#[gpui::test]`: toggling a switch commits immediately; reset removes the key.

---

## PUI-044 Config version and persistent workspaces

- **Requirements:** S3 (Should)
- **Crate:** planeui-core + planeui-app
- **Depends on:** PUI-042
- **Size:** M

### Scope

- Show `config-version` (absent = 1). "Upgrade to 2" sets it, with a note on the v1 → v2
  behaviour change: in v2 `persistent-workspaces` is explicit, in v1 it is inferred from
  bindings. Offer to prefill `persistent-workspaces` with the workspaces v1 inferred (names from
  `workspace` commands in bindings) in the same commit, so the upgrade does not change
  behaviour.
- Editable ordered list for `persistent-workspaces` (add, remove, reorder); array style and
  comments preserved.

### Acceptance criteria

- [ ] Upgrade + prefill is one undo entry.
- [ ] insta diffs for list add/remove/reorder.

---

## PUI-045 First usable build

- **Requirements:** milestone gate
- **Depends on:** PUI-036, PUI-041, PUI-043
- **Size:** S

### Scope

- Run the full AGENTS.md check list; fix anything outstanding.
- Sander uses PlaneUI on his real config for routine edits. Every bug found gets a ticket and a
  failing test first.
- Spot-check with `git diff` in Sander's dotfiles repo that every PlaneUI write changed only the
  intended lines.

### Acceptance criteria

- [ ] One working day of real use without a broken reload or a lost comment.
