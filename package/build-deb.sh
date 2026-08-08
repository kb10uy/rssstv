#!/usr/bin/env bash

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml" | head -n 1)"

RSSSTV_VERSION="v$version" bash "$root/docs/help/build.sh"
cd "$root"
cargo deb -p rssstv -- --locked
