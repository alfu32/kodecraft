#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bash "$task_root/scripts/build-jdbc-helper.sh" "$@"
export KODE_TEST_JDBC_HELPER="$task_root/jdbc-bridge/target/kode-jdbc-bridge.jar"
export KODE_TEST_H2_JAR="${KODE_JDBC_BUILD_CACHE:-/tmp/kode-rs-jdbc-maven-repository}/com/h2database/h2/2.2.224/h2-2.2.224.jar"
cargo test --manifest-path "$task_root/Cargo.toml" -p kode-database --test bridge --test drivers --test connections --test execution --test metadata --test console --test statement -- --include-ignored
cargo test --manifest-path "$task_root/Cargo.toml" -p kode-tui --test database_app --test database_connection_dialog --test database_explorer --test database_services --test database_results --test jdbc_qualification -- --include-ignored
