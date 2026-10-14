#!/usr/bin/env bash
# 第二批扩展门禁；P11–P16 的网络执行/ACK 尚未实现。
set -euo pipefail
rustfmt --edition 2021 --check src/catalog/mod.rs
git diff --check f9f02aa
git diff --exit-code 7b48182 -- src/newapi.rs src/newapi src/main.rs src/orchestrator.rs src/quota.rs src/boot.rs src/status.rs src/model_catalog.rs src/config.rs src/catalog/schema_v1.sql src/catalog/legacy.rs

sed -n '/pub fn open(path:/,/^    }/p' src/catalog/store.rs | rg '// Step P8:' >/dev/null
for step in 9 10; do
  sed -n '/pub fn submit_operation(/,/^    }/p' src/catalog/control.rs | rg "// Step P${step}:" >/dev/null
done
for name in OperationKind OperationPhase TargetType StatusDirective CreateState ControlErrorCode; do
  test "$(rg -l "control_enum!\($name" src | wc -l)" -eq 1
done
if rg 'reqwest|\.send\(|async fn|tokio' src/catalog/control.rs src/catalog/store.rs; then
  printf 'Catalog must not execute network operations\n' >&2
  exit 1
fi

operation_schema=docs/design/source-groups-token-management/schemas/control-operation.schema.json
test_output="$(cargo test --locked --offline control_operation_schema_output -- --nocapture)"
views="$(printf '%s\n' "$test_output" | sed -n 's/^CONTROL_SCHEMA_JSON=//p')"
enums="$(printf '%s\n' "$test_output" | sed -n 's/^CONTROL_ENUM_JSON=//p')"
test -n "$views"
test -n "$enums"
while IFS= read -r view; do
  check-jsonschema --schemafile "$operation_schema" --force-filetype json <(printf '%s\n' "$view")
done < <(printf '%s\n' "$views" | jq -c '.[]')
jq -en --argjson actual "$enums" --slurpfile spec "$operation_schema" '
  $actual.kind == $spec[0].properties.kind.enum and
  $actual.phase == $spec[0].properties.phase.enum and
  $actual.target_type == $spec[0].properties.target_type.enum and
  $actual.error_code == ($spec[0].properties.error_code.enum | map(select(. != null)))' >/dev/null
for field in kind phase target_type status_directive create_state error_code; do
  values="$(printf '%s\n' "$enums" | jq -r --arg field "$field" '.[$field] | map("\u0027" + . + "\u0027") | join(", ")')"
  rg -F "$field IN ($values)" src/catalog/schema_v2.sql >/dev/null
done
for filter in '.phase="invalid"' '.target_id="0"' '.target_revision=0' '.kind="retire_member"' '.credential="fake-only"' '.phase="succeeded" | .error_code="storage"'; do
  if check-jsonschema --quiet --schemafile "$operation_schema" --force-filetype json <(printf '%s\n' "$views" | jq ".[0] | $filter") >/dev/null; then
    printf 'Control schema accepted invalid case: %s\n' "$filter" >&2
    exit 1
  fi
done
snapshot_output="$(cargo test --locked --offline catalog_schema_output -- --nocapture)"
snapshot="$(printf '%s\n' "$snapshot_output" | sed -n 's/^CATALOG_SCHEMA_JSON=//p')"
check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/catalog-snapshot.schema.json \
  --force-filetype json <(printf '%s\n' "$snapshot")
printf 'sgm-2 batch 2 mechanical checks passed (database/intents only)\n'
