# Backlog

Could-haves from the PRD and ideas from the SPEC that the PRD moved out of v1. Not scheduled.

## PUI-090 Search and filter bindings

- **Requirements:** K8 (Could)
- Filter the bindings table by key, modifier or command text across all modes. Pure filter in
  core over the bindings view; a search field above the mode tabs.

## PUI-091 Undo history panel

- **Requirements:** U5 (Could)
- Side panel listing session changes by their `History` labels (PUI-017), "Added alt-shift-p in
  main". Clicking an entry jumps to that state through the pipeline as a single write.

## PUI-092 Rule from a window

- **Requirements:** W5 (Could)
- "New rule from a window" lists `list-windows --all --json` and prefills app bundle id and
  window title. Needs `list_windows` on the `Aerospace` trait.

## Post-v1 ideas (from SPEC, not in PRD v1)

Each needs a PRD change first.

- Callbacks, `after-startup-command` and `exec` env editors.
- Raw TOML tab.
- Keyboard heatmap / cheat sheet per mode.
- Try-with-auto-revert (apply, keep within 15 s or roll back).
- Socket client and `subscribe` event stream instead of subprocesses.
- Import/export snippets and presets.
