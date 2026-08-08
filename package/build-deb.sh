#!/usr/bin/env bash
# Builds the Debian package. The manual is rendered first because cargo-deb
# collects target/help as an asset but runs no build steps of its own. Runs
# from any directory; cargo itself runs from the repository root so that
# .cargo/config.toml (the mold linker flag) is picked up.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)"

RSSSTV_VERSION="v$version" bash "$root/docs/help/build.sh"
cd "$root"
cargo deb -p rssstv -- --locked
