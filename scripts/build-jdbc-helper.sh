#!/usr/bin/env bash
set -euo pipefail
task_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
task_repo="${KODE_JDBC_BUILD_CACHE:-/tmp/kode-rs-jdbc-maven-repository}"
# Isolate build settings/cache; never read the user's Maven credentials/settings.
export MAVEN_SKIP_RC=true
export MAVEN_OPTS=
export MAVEN_ARGS=
export JAVA_TOOL_OPTIONS=
export _JAVA_OPTIONS=
export JDK_JAVA_OPTIONS=
exec mvn -f "$task_root/jdbc-bridge/pom.xml" --batch-mode --no-transfer-progress \
  --settings "$task_root/jdbc-bridge/settings.xml" --global-settings "$task_root/jdbc-bridge/settings.xml" \
  --toolchains "$task_root/jdbc-bridge/toolchains.xml" --global-toolchains "$task_root/jdbc-bridge/toolchains.xml" \
  "-Dmaven.repo.local=$task_repo" -Dstyle.color=never verify "$@"
