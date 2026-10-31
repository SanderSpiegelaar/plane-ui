# M5 Polish

Goal: a proper Mac app installed as `PlaneUI.app`, meeting the NFRs, after a week of daily use.
The shell itself already exists (PUI-021); M5 adds menus, badges, packaging and the NFR checks.

---

## PUI-060 Native menu bar

- **Requirements:** A1
- **Crate:** planeui-app
- **Depends on:** PUI-019
- **Size:** S

### Scope

- `cx.set_menus`: PlaneUI (About, Quit), Edit (Undo, Redo, Cut, Copy, Paste, Select All) and
  Window (Minimize, Zoom). Undo/Redo dispatch the existing actions; text fields keep their own
  undo while focused (check gpui-kit input behaviour and document the choice).
- No File > Save (F4).

### Acceptance criteria

- [ ] Menu Undo/Redo behave exactly like Cmd-Z / Cmd-Shift-Z.

---

## PUI-061 Section error badges and "Other settings in your file"

- **Requirements:** A2, A4, F7
- **Crate:** planeui-app
- **Depends on:** PUI-021, PUI-011
- **Size:** S

### Scope

- Sidebar badge on any section with an invalid field or a validation error in its keys.
- "Other settings in your file" list (in General, or its own sidebar entry) naming each
  `out_of_scope` key with its line number and "kept as is".

### Acceptance criteria

- [ ] `#[gpui::test]`: an invalid gap value badges Gaps and clears when fixed.

---

## PUI-062 Keyboard navigation and theme audit

- **Requirements:** NFR Usability
- **Crate:** planeui-app
- **Size:** M

### Scope

- Walk every screen with keyboard only (Tab order, focus rings, Enter/Escape in forms,
  shortcuts for sidebar sections such as `cmd-1`…`cmd-6`).
- Verify light and dark mode on every screen; no hard-coded colors (`grep` for color literals).
- File tickets for gaps found; fix the small ones here.

---

## PUI-063 App bundle and icon

- **Requirements:** A1
- **Crate:** packaging
- **Size:** S–M

### Scope

- Build `PlaneUI.app` with `cargo-packager` (a tool, not a dependency): bundle id, icon,
  `LSApplicationCategoryType`, minimum macOS version. Ad-hoc signed only; no notarisation.
- `scripts/install.sh` builds release and copies to `~/Applications`.
- Icon: simple, matches macOS style.

### Acceptance criteria

- [ ] Launching from Finder/Dock works, including finding `aerospace` without a shell PATH.

---

## PUI-064 Error resilience pass

- **Requirements:** NFR Robustness
- **Crate:** both
- **Size:** S

### Scope

- Review every `Task` for dropped results; every async failure reaches the UI.
- Grep for `unwrap`/`expect`/indexing outside tests; confirm none.
- Manually exercise: AeroSpace quit mid-session, config deleted while open, Application Support
  not writable, config made read-only.

---

## PUI-065 Performance check

- **Requirements:** NFR Latency, NFR Startup
- **Crate:** both
- **Size:** S

### Scope

- Measure commit → "Applied" (target < 500 ms incl. snapshot) and launch → config loaded
  (target < 1 s) on Sander's Mac using the PUI-018 step timings in release builds.
- Fix or file tickets for misses.

---

## PUI-066 Week of daily use

- **Requirements:** PRD success criteria
- **Size:** 1–2 days of fixes spread over the week

### Scope

- Sander uses PlaneUI for all AeroSpace config changes for a week. Each issue → failing test →
  fix.
- Final check: no network access (review dependency tree), `cargo deny check` clean, README
  with build/install instructions.

### Acceptance criteria

- [ ] PRD success criteria hold over the week: zero lost comments, no broken reloads.
