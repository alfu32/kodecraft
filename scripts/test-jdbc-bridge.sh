#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bash "$task_root/scripts/build-jdbc-helper.sh" "$@"
export KODE_TEST_JDBC_HELPER="$task_root/jdbc-bridge/target/kode-jdbc-bridge.jar"
cargo test --manifest-path "$task_root/Cargo.toml" -p kode-database --test bridge -- --include-ignored
