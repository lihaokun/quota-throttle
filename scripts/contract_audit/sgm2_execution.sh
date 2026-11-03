#!/usr/bin/env bash
# 持久执行日志小步；不将 worker/回读确认/真实 HTTP 交付证据记为已实现。
set -euo pipefail
rustfmt --edition 2021 --check src/catalog/mod.rs src/reconcile/mod.rs
git diff --check 7c5efcc
git diff --exit-code 7c5efcc -- src/main.rs src/newapi.rs src/newapi src/orchestrator.rs src/quota.rs src/boot.rs src/status.rs src/model_catalog.rs src/config.rs src/catalog/schema_v1.sql src/catalog/schema_v2.sql src/catalog/legacy.rs Cargo.toml Cargo.lock
for pair in 'prepare_execution 13' 'settle_execution 14' 'recover_executions 16'; do
  read -r name step <<< "$pair"
  sed -n "/fn $name(/,/^    }/p" src/catalog/execution.rs | rg "// Step P${step}:" >/dev/null
done
if rg 'reqwest|\.send\(|async fn|tokio' src/catalog/execution.rs; then
  printf 'Execution journal must not perform network IO\n' >&2
  exit 1
fi
for name in OperationKind OperationPhase TargetType StatusDirective CreateState ControlErrorCode; do
  test "$(rg -l "control_enum!\($name" src | wc -l)" -eq 1
done

operation_schema=docs/design/source-groups-token-management/schemas/control-operation.schema.json
output="$(cargo test --locked --offline control_operation_schema_output -- --nocapture)"
views="$(printf '%s\n' "$output" | sed -n 's/^CONTROL_SCHEMA_JSON=//p')"
enums="$(printf '%s\n' "$output" | sed -n 's/^CONTROL_ENUM_JSON=//p')"
test -n "$views"
while IFS= read -r view; do
  check-jsonschema --schemafile "$operation_schema" --force-filetype json <(printf '%s\n' "$view")
done < <(printf '%s\n' "$views" | jq -c '.[]')
errors="$(printf '%s\n' "$enums" | jq -r '.error_code | map("\u0027" + . + "\u0027") | join(", ")')"
rg -F "error_code IN ($errors)" src/catalog/schema_v3.sql >/dev/null
output="$(cargo test --locked --offline catalog_schema_output -- --nocapture)"
snapshot="$(printf '%s\n' "$output" | sed -n 's/^CATALOG_SCHEMA_JSON=//p')"
check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/catalog-snapshot.schema.json --force-filetype json <(printf '%s\n' "$snapshot")
printf 'SGM-2 persistent journal checks passed; worker and readback completion remain pending\n'
