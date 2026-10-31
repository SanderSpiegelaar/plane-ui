# M0 Spike

Goal: prove the two riskiest assumptions (gpui-kit builds and opens a window on Sander's Mac;
`toml_edit` round-trips real AeroSpace configs byte-for-byte) on top of the workspace layout
AGENTS.md prescribes. Size: 1.5–2 days.

---

## PUI-001 Convert to the AGENTS.md workspace layout

- **Requirements:** NFR Maintainability, NFR Privacy
- **Crate:** workspace
- **Depends on:** —
- **Size:** S

### Goal

Replace the single `plane-ui` package with the workspace described in AGENTS.md so every later
ticket has its home.

### Scope

- Root `Cargo.toml` with `[workspace]`, `resolver = "3"`, members `crates/planeui-core` and
  `crates/planeui-app`, `[workspace.package]` (`edition = "2024"`, `rust-version = "1.99"`),
  `[workspace.dependencies]` and `[workspace.lints]` exactly as listed in AGENTS.md.
- `rust-toolchain.toml` pinned to `1.99.1` with `rustfmt` and `clippy`.
- `clippy.toml` with `allow-unwrap-in-tests` and `allow-expect-in-tests`.
- `deny.toml`: permissive licenses only (MIT, Apache-2.0, BSD-2/3, ISC, Unicode-3.0, Zlib),
  advisories denied, duplicate majors warned, only crates.io as a source. Ban crates that open
  network sockets we do not need (e.g. `reqwest`, `hyper`) to back NFR Privacy.
- `planeui-core`: `lib.rs` with empty module files named in AGENTS.md (`config_doc`,
  `config_view`, `validate`, `keys`, `commands`, `store`, `aerospace`), deps `toml_edit`,
  `thiserror`, `log`, `serde`, `serde_json`, `dirs`; dev-deps `tempfile`, `insta`, `proptest`,
  `similar`.
- `planeui-app`: binary named `PlaneUI`, depends on `planeui-core`, `gpui-kit = "=0.8.1"` (exact
  pin; the current `"0.8.1"` is a caret range), `anyhow`, `log`.
- Delete `src/main.rs` and the old package. Commit `Cargo.lock`.

### Acceptance criteria

- [ ] `cargo build --workspace --locked` succeeds.
- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`
      and `cargo deny check` pass.
- [ ] `cargo test -p planeui-core` builds with no GPUI crate in its dependency tree
      (`cargo tree -p planeui-core | grep -i gpui` is empty).

---

## PUI-002 GPUI window spike

- **Requirements:** A1 (groundwork)
- **Crate:** planeui-app
- **Depends on:** PUI-001
- **Size:** S–M

### Goal

A regular Dock app window opens through gpui-kit, proving Xcode/Metal toolchain and the pinned
gpui-kit API on Sander's Mac.

### Scope

- `main.rs`: `gpui_kit::application().run(..)`, `gpui_kit::init(cx)`, `gpui_kit::open_window`
  with a resizable window titled "PlaneUI" rendering a placeholder root view that uses
  `cx.theme()` colors and one gpui-kit component (e.g. a button) to prove theming works.
- Register a quit action bound to `cmd-q` so the app behaves like a normal Mac app.
- Read the pinned gpui-kit source for exact signatures (do not trust memory or older Zed code);
  write a short note in the PR on any API surprises.
- Initialize `log` with a simple logger (decide `env_logger` vs `simplelog` with Sander first, or
  use gpui-kit's own if it ships one).

### Acceptance criteria

- [ ] `cargo run -p planeui-app` opens a window with a Dock icon; Cmd-Q quits.
- [ ] Window follows system light/dark mode.
- [ ] No `println!`; clippy clean.

### Out of scope

Menus (PUI-060), shell layout (PUI-021).

---

## PUI-003 Fixture corpus and lossless round-trip

- **Requirements:** F2, NFR Fidelity
- **Crate:** planeui-core
- **Depends on:** PUI-001
- **Size:** M

### Goal

Prove `toml_edit::DocumentMut` round-trips every config shape we care about, and lay down the
fixture corpus that every later test reuses.

### Scope

- `ConfigDoc` in `config_doc.rs`: `ConfigDoc::parse(&str) -> Result<ConfigDoc, ParseError>`,
  `ConfigDoc::to_string()` / `Display`. `ParseError` carries line and column from the
  `toml_edit` span.
- Fixtures in `crates/planeui-core/tests/fixtures/`:
  - `aerospace-default.toml`: AeroSpace's `default-config.toml` for the pinned 0.21.x release
    (MIT; keep its header). Also used by PUI-022 to create a new config.
  - `sander.toml`: Sander's real config, copied unchanged (roadmap D5).
  - `heavily-commented.toml`, `dotted-keys.toml` (root `gaps.inner.horizontal = …`),
    `table-style.toml` (`[gaps.inner]`), `inline-tables.toml`,
    `rules-array-of-tables.toml` (`[[on-window-detected]]`), `rules-inline-array.toml`
    (`on-window-detected = [ … ]` with TOML 1.1 multi-line inline tables),
    `legacy-rules.toml` (`if.app-id`), `per-monitor-gaps.toml`, `crlf.toml` (CRLF endings, check
    it is committed with CRLF: add a `.gitattributes` entry `*.crlf.toml -text` or similar),
    `unicode-and-quoting.toml` (literal vs basic strings, escapes, unicode keys),
    `callbacks-and-exec.toml` (sections PlaneUI never edits).
- `tests/round_trip.rs`: for every file in `fixtures/`, `parse(file).to_string() == file`.

### Acceptance criteria

- [ ] Round-trip test iterates the directory, so new fixtures are covered automatically.
- [ ] All fixtures pass byte-for-byte, including CRLF and TOML 1.1 multi-line inline tables.
- [ ] Any fixture that does not round-trip is a blocker reported to Sander, not skipped.

---

## PUI-004 GitHub Actions CI

- **Requirements:** NFR Maintainability
- **Crate:** repo
- **Depends on:** PUI-001
- **Size:** S

### Goal

A CI run that executes the full AGENTS.md check list, so "done" is mechanical.

### Scope

Per roadmap D4: a GitHub Actions workflow on push and pull request. Linux job: `fmt`,
`clippy`, `test` for `planeui-core` with `--locked`. macOS job: the full workspace plus
`cargo deny check`. Cache the cargo registry and `target/`.

### Acceptance criteria

- [ ] A deliberately introduced clippy warning fails the check.
- [ ] `planeui-core` checks run on Linux.
