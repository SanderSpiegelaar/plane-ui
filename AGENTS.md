# AGENTS.md

Instructions for AI coding agents working in the PlaneUI repository. Read this whole file before
changing code. Product intent lives in the PRD (`docs/PlaneUI-PRD.md`) and the technical basis in
the spec (`docs/PlaneUI-Spec.md`); when this file and those disagree on behavior, the PRD wins.

## Project overview

- PlaneUI is a personal macOS desktop app (regular Dock app, normal window) that edits the
  AeroSpace tiling window manager config (`aerospace.toml`) through forms.
- Stack: Rust (edition 2024), GPUI via Longbridge's `gpui-kit`, `toml_edit` for lossless TOML.
- Every valid change is saved instantly and followed by `aerospace reload-config --no-gui`.
  There is no Save button and no Cmd-S.
- v1 edits: keybindings per mode, gaps (global; per-monitor gaps arrive in M4), general settings,
  window rules (`on-window-detected`), workspace-to-monitor assignment.
- Out of scope for v1: startup commands, `exec` env, free-text command editing, a key recorder.
  Do not add them, even "quickly". Leave those parts of the file untouched.

## Project Documentation

- You can find the PRD in `./docs/PRD.md`
- You can find the Project Spec in `./docs/SPEC.md`

## Workspace layout

```text
Cargo.toml              # [workspace], shared deps, [workspace.lints]
rust-toolchain.toml     # pinned toolchain
deny.toml               # cargo-deny policy
clippy.toml
crates/
  planeui-core/         # platform-independent: no GPUI, no AppKit, no global state
    src/config_doc.rs   # toml_edit DocumentMut wrapper; all mutations go through here
    src/config_view.rs  # typed read-only view built from the document
    src/validate.rs     # syntax / schema / semantic / lint checks
    src/keys.rs         # key + modifier names, key-mapping aliases, conflict detection
    src/commands.rs     # AeroSpace command catalogue + structured arguments
    src/store.rs        # path resolution, atomic write, snapshots (keep 10)
    src/aerospace.rs    # `Aerospace` trait + CLI impl (reload, list-apps, list-monitors)
    tests/              # round-trip, snapshot, property tests; fixtures in tests/fixtures/
  planeui-app/          # GPUI binary "PlaneUI": windows, views, actions, wiring only
```

- Keep `planeui-core` free of any GPUI or macOS dependency. It must build and test on Linux.
- Keep `planeui-app` thin: views call into core; domain logic and validation never live in views.
- Talk to AeroSpace only through the `Aerospace` trait so tests use a fake. Never call the real
  CLI from tests.
- Name files after modules (`src/foo.rs`); do not create `mod.rs` files.
- Add a new crate only when a boundary has a concrete benefit; ask first.

## Toolchain and Cargo conventions

- Use edition 2024 and `resolver = "3"`. Pin the toolchain in `rust-toolchain.toml`
  (currently `1.99.1`, components `rustfmt`, `clippy`) and set `rust-version` in
  `[workspace.package]` to the same minor. This is a personal app: MSRV = pinned toolchain.
- Declare shared dependency versions once in `[workspace.dependencies]`; member crates use
  `foo.workspace = true`. Declare lints once in `[workspace.lints]`; members use
  `[lints] workspace = true`.
- Commit `Cargo.lock`. Run Cargo with `--locked` in CI.
- Depend on `gpui-kit` only (it pins the matching `gpui-pre` snapshot and re-exports GPUI as
  `use gpui_kit::*;`). Never add `gpui`, `gpui-pre`, or `gpui-component` directly.

## Commands

Run all of these before declaring a task done; they must pass with zero warnings.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check                      # licenses, advisories, bans, sources
cargo insta test --review             # when snapshots change; review every diff
cargo run -p planeui-app              # manual check, macOS only
cargo test -p planeui-core            # fast loop; works on any OS
```

- Do not "fix" a failing lint with `#[allow]`. If a lint is genuinely wrong for a spot, use
  `#[expect(clippy::lint_name, reason = "...")]` with a real reason.
- Never accept snapshot changes blindly (`cargo insta accept`) without reading each diff.

## Lint configuration (keep in sync with Cargo.toml)

```toml
[workspace.lints.rust]
unsafe_code = "forbid"
missing_debug_implementations = "warn"
unused_must_use = "deny"

[workspace.lints.clippy]
all = { level = "warn", priority = -1 }
pedantic = { level = "warn", priority = -1 }
module_name_repetitions = "allow"
unwrap_used = "deny"
expect_used = "warn"
panic = "deny"
indexing_slicing = "warn"
dbg_macro = "deny"
todo = "deny"
print_stdout = "deny"
print_stderr = "deny"
let_underscore_must_use = "deny"
```

`clippy.toml`: `allow-unwrap-in-tests = true`, `allow-expect-in-tests = true`.

## Code style

- Use default `rustfmt` settings; do not add a custom `rustfmt.toml` without asking.
- Prioritize correctness and clarity over speed unless a profile says otherwise.
- Follow the Rust API Guidelines: `snake_case` functions, `UpperCamelCase` types, `as_`/`to_`/`into_`
  conversion naming, `new` constructors, derive `Debug` (and `Clone`, `PartialEq`, `Eq` where
  sensible) on public types, use newtypes for domain values (`ModeName`, `WorkspaceName`, `AppId`).
- Use full words for names (`binding`, not `b`; `monitor`, not `mon`).
- Make invalid states unrepresentable: enums over strings/bools for modes, layouts, orientations.
- Comment only _why_, never _what_. Doc-comment every public item in `planeui-core`.
- Keep functions small and pure where possible; push I/O to the edges (`store.rs`, `aerospace.rs`).
- Prefer editing existing files over creating many tiny ones.

## Error handling

- In `planeui-core`, define typed errors with `thiserror` (one enum per module boundary, e.g.
  `LoadError`, `ValidationError`, `WriteError`, `ReloadError`). Include context (path, key path,
  line/column from `toml_edit` spans) so the UI can show an inline message.
- In `planeui-app`, `anyhow::Result` is fine for glue; attach context with `.context(...)`.
- Never `unwrap()`/`expect()`/`panic!` in non-test code. Never index without a bounds guarantee.
- Never silently discard errors (`let _ = fallible()`). Propagate with `?`, show them in the UI,
  or log them explicitly. Async failures must reach the UI so the user sees what happened.
- Use `log` macros for diagnostics; never `println!`.

## Config-safety invariants (non-negotiable)

These come from the PRD. Any change touching load, edit, or write must preserve all of them and
must ship with tests proving it.

1. **Never write invalid config.** Pipeline per change: mutate an in-memory copy -> validate
   (TOML syntax, schema, semantics such as unknown commands/keys, binding conflicts) -> only if
   valid, write. Invalid edits stay in memory and show an inline error until fixed.
2. **Preserve unknown content byte-for-byte.** Edit through `toml_edit::DocumentMut` only; never
   re-serialize from typed structs (`toml`/serde) and never reformat untouched tables, comments,
   whitespace, key order, or dotted-vs-table style. Sections PlaneUI does not edit (startup
   commands, `exec`, legacy `if.app-id` rules, unknown keys) are never rewritten.
3. **Snapshot before every write; keep 10.** Copy the current on-disk file to
   `~/Library/Application Support/PlaneUI/snapshots/<timestamp>.toml` before writing, then delete
   the oldest beyond 10. If the snapshot fails, do not write.
4. **Write atomically.** Write a temp file in the same directory, `sync_all`, then `rename` over
   the target. If the config path is a symlink, write to the resolved target and keep the link.
5. **Detect external edits.** Remember the content hash from the last load/write; if the file on
   disk changed, do not clobber it: reload it and tell the user.
6. **Reload after every write.** Run `aerospace reload-config --no-gui`, parse its output, and
   show success or AeroSpace's errors. A missing/failed CLI is a visible error, not a panic.
7. **Respect path rules.** Resolve `~/.aerospace.toml` and
   `${XDG_CONFIG_HOME:-~/.config}/aerospace/aerospace.toml`; if both exist, refuse to edit and
   explain (AeroSpace treats that as ambiguous).
8. **Undo = a new write.** Cmd-Z / Cmd-Shift-Z apply a prior document state through the same
   validate -> snapshot -> write -> reload pipeline. Restoring a snapshot does the same.
9. **`auto-reload-config`.** When it is `true`, offer (never force) to set it to `false`.
10. **Commit cadence.** Toggles, pickers, and dropdowns commit immediately. Text and number
    inputs commit on blur/Enter or after a short debounce (~500 ms); never reload per keystroke.
    Coalesce bursts into one write + one reload.

## Testing requirements

- **Round-trip:** for every fixture in `crates/planeui-core/tests/fixtures/` (include AeroSpace's
  default config, heavily commented files, dotted keys, inline tables, CRLF, TOML 1.1 features),
  assert `load(file).to_string() == file` byte-for-byte.
- **Minimal diffs:** for each editor operation, snapshot the resulting file or unified diff
  with `insta` and assert that only the targeted lines changed.
- **Property tests (`proptest`):** generate random valid edits/sequences; assert the output
  re-parses, validates, preserves every untouched table verbatim, and undo restores the original
  bytes exactly.
- **Validation:** table-driven tests for each error class (bad key name, unknown command, bad
  argument, duplicate binding, unknown mode reference) with expected messages.
- **Store:** use `tempfile` dirs to test atomic write, snapshot rotation (11th write deletes the
  oldest), symlink handling, external-change detection, and "snapshot failed -> no write".
- **AeroSpace:** test reload/list parsing against recorded CLI output with a fake `Aerospace`.
- **GPUI:** use `#[gpui::test]` / `TestAppContext` for view logic (debounce, undo, inline error
  state). Use GPUI executor timers (`cx.background_executor().timer(..)`), not `smol::Timer`.
- Every bug fix starts with a failing test. Do not add tests that only assert pixel sizes.

## GPUI guidance

- Start the app with `gpui_kit::application().run(..)`, call `gpui_kit::init(cx)` before opening
  windows, and open windows with `gpui_kit::open_window`. Use gpui-kit components (inputs,
  checkbox, select, table, list, notifications) and theme tokens via `cx.theme()`; do not
  hand-roll widgets or hard-code colors.
- Hold state in entities (`Entity<T>` created with `cx.new`). One `ConfigModel` entity owns the
  `ConfigDoc`, undo stack, validation state, and save status; views observe/subscribe to it.
- Implement `Render` for stateful views and `RenderOnce` + `#[derive(IntoElement)]` for small
  stateless components. Use `SharedString` for retained UI text.
- Call `cx.notify()` after any state change that affects rendering. Emit typed events with
  `cx.emit` (`impl EventEmitter<..>`) and store `Subscription`s in a `_subscriptions` field.
- Inside `entity.update(cx, |this, cx| ..)`, use the inner `cx`. Never update an entity that is
  already being updated (it panics). Use `WeakEntity` for back-references to avoid cycles.
- Define user commands as actions (`actions!(planeui, [Undo, Redo, ...])`, or the `Action`
  derive for actions with data), bind keys with `KeyBinding::new("cmd-z", Undo, None)`, and
  handle them with `.on_action(cx.listener(..))`.
- **Never block the UI thread.** All file I/O, snapshotting, hashing, and every `aerospace`
  invocation run in `cx.background_spawn(..)`; apply results back with `this.update(cx, ..)`
  from a `cx.spawn(async move |this, cx| ..)` task. No `std::fs`, `std::process::Command`, or
  `block_on` inside `render`, event handlers, or entity updates.
- A dropped `Task` is cancelled: store debounce/save tasks in a field (replacing the field is how
  debouncing cancels the previous one), await them, or `detach_and_log_err(cx)`; never drop
  silently.
- Serialize writes: at most one write/reload pipeline in flight; queue the latest state.
- Shadow clones before `async move` blocks (`let model = model.clone();`) to keep scopes clear.
- GPUI APIs churn between snapshots: check the pinned `gpui-kit` source/docs for exact
  signatures instead of guessing from memory or older Zed code.

## Dependency policy

- Ask before adding any dependency. Prefer std, then crates already in the tree.
- Expected set: `gpui-kit`, `toml_edit`, `thiserror`, `anyhow` (app only), `log`, `serde` +
  `serde_json` (CLI `--json` output), `dirs`, `tempfile`/`insta`/`proptest`/`similar` (dev).
- Pin to the latest stable release at time of adding; no git dependencies except the pinned
  `gpui-kit` when crates.io lags, and then pin a `rev`.
- `cargo deny check` must pass: allow only permissive licenses (MIT, Apache-2.0, BSD, ISC,
  Unicode, Zlib), deny known advisories, deny duplicate major versions where avoidable.
- No `unsafe`, no `build.rs`, no proc-macro crates of our own without approval.

## Commits and pull requests

- One logical change per commit. Commit messages: imperative, capitalized, no trailing period,
  no `feat:`/`fix:` prefixes; optional crate scope (`core: Preserve comments when removing a
  binding`). Body explains why.
- PR description: what changed, which PRD requirement it serves, how it was tested (commands run,
  fixtures added), and screenshots for UI changes.
- Never commit secrets, real personal config files, or generated snapshots you have not reviewed.
- Do not edit the PRD, spec, or this file as a side effect of feature work; propose changes in
  the PR description under "Suggested AGENTS.md changes".
- Never run destructive git commands (force-push, `reset --hard`, history rewrite) without asking.

## Sources

- AGENTS.md convention: https://agents.md
- Rust API Guidelines: https://rust-lang.github.io/api-guidelines/checklist.html
- Rust 2024 edition guide: https://doc.rust-lang.org/edition-guide/rust-2024/index.html
- Cargo workspaces, `[lints]`, `rust-version`: https://doc.rust-lang.org/cargo/reference/workspaces.html,
  https://doc.rust-lang.org/cargo/reference/manifest.html#the-lints-section
- Clippy lints and config: https://rust-lang.github.io/rust-clippy/master/index.html,
  https://doc.rust-lang.org/clippy/lint_configuration.html
- rustfmt: https://github.com/rust-lang/rustfmt
- thiserror / anyhow: https://docs.rs/thiserror, https://docs.rs/anyhow
- insta / proptest: https://insta.rs/docs/, https://proptest-rs.github.io/proptest/
- cargo-deny: https://embarkstudios.github.io/cargo-deny/
- toml_edit: https://docs.rs/toml_edit
- Zed agent rules and GPUI notes: https://github.com/zed-industries/zed/blob/main/.rules
- gpui-kit AGENTS.md and coding guides: https://github.com/longbridge/gpui-kit/blob/main/AGENTS.md,
  https://github.com/longbridge/gpui-kit/blob/main/website/docs/coding-guides.md
- AeroSpace config and commands: https://nikitabobko.github.io/AeroSpace/guide,
  https://nikitabobko.github.io/AeroSpace/commands

<!-- REPOWISE_DISTILL:START — Do not edit below this line. Auto-generated by Repowise. -->
### Output Distillation

- Prefer `repowise distill <cmd>` for noisy commands: test runs, builds, `git status`/`log`/`diff`, searches, file listings. It runs the command unchanged (exit code preserved) and prints a compact, errors-first rendering; every error line survives.
- Output may contain a marker like `[repowise#a1b2c3d4e5f6: 230 lines omitted (~6.1k tokens); restore: repowise expand a1b2c3d4e5f6]`. The omitted content is fully preserved: run `repowise expand <ref>` to retrieve it, or `repowise expand <ref> -q <regex>` for just the matching lines.
- Never re-run a command to see omitted output; expand the marker instead.
- For structure-level questions about a large indexed file ("what's in here", "which function handles X"), `get_context(["path"], include=["skeleton"])` returns the file with bodies elided (every signature plus the bodies of the most central symbols) at a fraction of the cost of a full Read.
<!-- REPOWISE_DISTILL:END -->
