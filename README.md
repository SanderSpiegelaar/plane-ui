# PlaneUI

A macOS app for editing the AeroSpace window manager configuration through forms.

## Requirements

- macOS 13 or later
- Rust toolchain specified in `rust-toolchain.toml`
- [AeroSpace](https://nikitabobko.github.io/AeroSpace/guide)

## Build

```sh
cargo run -p planeui-app
```

## Install

```sh
cargo install cargo-packager --locked
scripts/install.sh
```

Installs an ad-hoc signed app at `~/Applications/PlaneUI.app`. Not notarised.

## Develop

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo deny check
cargo insta test --review  # When snapshots change; review every diff.
cargo run -p planeui-app   # Manual check on macOS.
cargo test -p planeui-core
```
