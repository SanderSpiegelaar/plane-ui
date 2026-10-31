# AeroSpace CLI output fixtures

Each directory is one `aerospace` invocation: `stdout`, `stderr`, and `exit-code`.
`tests/aerospace.rs` replays them through a fake binary, so no test runs the real CLI.

Recorded from AeroSpace 0.21.3-Beta on 31 Oct 2026:

- `version`: `aerospace --version`
- `config-path`: `aerospace config --config-path`
- `reload-applied`: `aerospace reload-config --dry-run --no-gui` on a valid config. A real
  reload of a valid config prints the same nothing; it was not run so the live config stayed put.
- `list-apps`: `aerospace list-apps --json`
- `list-monitors`: `aerospace list-monitors --json --format '%{monitor-id}%{monitor-name}%{monitor-is-main}'`
  with three monitors connected (two LG ULTRAGEAR+ and the built-in display, which is main).
  Plain `--json` omits `monitor-is-main`.
- `list-workspaces`: `aerospace list-workspaces --all --json`

Directories ending in `-synthesized` were written by hand, because recording them needs a
stopped server or a broken live config. Their formats follow the v0.21.3-Beta source:
`Sources/Cli/_main.swift` (not-running message, `--version` lines) and
`Sources/AppBundle/command/impl/ReloadConfigCommand.swift` with
`Sources/AppBundle/config/parseConfig.swift` (`[ERROR] <key path>: <message>` blocks joined by
blank lines on stdout, warnings on stderr, exit code 1 on errors). The `NWError` text after the
not-running message is a guess; PlaneUI only matches the line before it.
