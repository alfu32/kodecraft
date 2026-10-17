#!/usr/bin/env bash
set -euo pipefail
if [[ $# != 2 ]]; then
  echo 'usage: package.sh VERSION TARGET' >&2
  exit 2
fi
release_version=$1
release_target=$2
release_root=$(cd "$(dirname "$0")/../.." && pwd)
release_assets="$release_root/target/release-assets"
release_name="kode-$release_version-$release_target"
release_stage="$release_root/target/macos-package-$release_target"
rm -rf "$release_stage"
mkdir -p "$release_stage"
ditto -x -k "$release_assets/$release_name.zip" "$release_stage"
test -f "$release_stage/$release_name/jdbc/kode-jdbc-bridge.jar"
release_dmg="$release_assets/kode-$release_version-macos-${release_target%%-*}.dmg"
hdiutil create -quiet -volname "Kode $release_version" -srcfolder "$release_stage/$release_name" -format UDZO -ov "$release_dmg"
hdiutil verify -quiet "$release_dmg"
shasum -a 256 "$release_dmg" > "$release_dmg.sha256"
