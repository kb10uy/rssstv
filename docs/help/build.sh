#!/usr/bin/env bash
# Renders the bundled manual with pandoc. The output directory is what the
# release archive carries as `help/`.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
out="${1:-$here/../../target/help}"

pages=(index receive transmit rig files troubleshooting)

rm -rf "$out"
mkdir -p "$out"
cp "$here/style.css" "$out/"

for page in "${pages[@]}"; do
  pandoc "$here/$page.md" \
    --from markdown-smart+east_asian_line_breaks \
    --to html5 \
    --standalone \
    --template "$here/template.html" \
    --lua-filter "$here/link-to-html.lua" \
    --variable "current-$page=1" \
    ${RSSSTV_VERSION:+--variable "version=$RSSSTV_VERSION"} \
    --output "$out/$page.html"
done
