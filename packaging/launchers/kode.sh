#!/bin/sh
# The native executable discovers resources and the helper beside itself.
set -eu
kode_launcher_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd -P)
exec "$kode_launcher_dir/kode" "$@"
