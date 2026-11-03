#!/bin/sh
# Run from the project root after cargo build --locked --release.
set -eu
install_dir=${RR_INSTALL_DIR:-./bin}
source_binary=./target/release/release-radar
if [ ! -x "$source_binary" ]; then
  printf '%s\n' 'Build first: cargo build --locked --release' >&2
  exit 1
fi
mkdir -p "$install_dir"
staged_binary=$(mktemp "$install_dir/.release-radar.XXXXXX")
trap 'rm -f "$staged_binary"' EXIT HUP INT TERM
install -m 755 "$source_binary" "$staged_binary"
if [ -f "$install_dir/release-radar" ]; then
  cp -p "$install_dir/release-radar" "$install_dir/release-radar.previous"
fi
mv -f "$staged_binary" "$install_dir/release-radar"
printf '%s\n' "Installed $install_dir/release-radar. Restart your service when ready."
