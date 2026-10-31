#!/usr/bin/env bash
set -euo pipefail

if ! cargo packager --version >/dev/null 2>&1; then
    echo "Install cargo-packager first: cargo install cargo-packager --locked" >&2
    exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"
# cargo-packager otherwise notarises automatically when Apple credentials are present.
unset APPLE_KEYCHAIN_PROFILE APPLE_ID APPLE_API_KEY
cargo packager --release --packages planeui-app --out-dir "$repo_root/target/release"

mkdir -p "$HOME/Applications"
rm -rf "$HOME/Applications/PlaneUI.app"
cp -R "$repo_root/target/release/PlaneUI.app" "$HOME/Applications/PlaneUI.app"
echo "Installed $HOME/Applications/PlaneUI.app"
