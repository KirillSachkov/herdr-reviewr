#!/usr/bin/env bash
# Put a release build of the own fork into the own linked plugin (own/herdr-plugin).
# A fresh inode and an ad-hoc signature, as qa-install does: macOS kills a binary overwritten in place.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
plugin="$root/own/herdr-plugin"
cargo build --release --manifest-path "$root/Cargo.toml"
mkdir -p "$plugin/bin"
tmp="$plugin/bin/.herdr-reviewr.new"
cp "$root/target/release/herdr-reviewr" "$tmp"
[[ "$(uname)" == Darwin ]] && codesign --force --sign - "$tmp"
mv -f "$tmp" "$plugin/bin/herdr-reviewr"
# herdr reads the manifest at link time, so a changed manifest links again (validated first).
stamp="$plugin/bin/.manifest.sha"
sum="$(shasum "$plugin/herdr-plugin.toml" | cut -d' ' -f1)"
if ! herdr plugin list 2>/dev/null | grep -q '^- kirill.reviewr '; then
  herdr plugin link "$plugin" >/dev/null
elif [[ "$(cat "$stamp" 2>/dev/null)" != "$sum" ]]; then
  herdr plugin unlink kirill.reviewr >/dev/null && herdr plugin link "$plugin" >/dev/null
fi
echo "$sum" > "$stamp"
echo "installed: reopen reviewr-own panes to load the new binary"
