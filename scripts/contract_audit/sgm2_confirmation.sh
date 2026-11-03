#!/usr/bin/env bash
set -euo pipefail
rustfmt --edition 2021 --check src/catalog/mod.rs src/reconcile/mod.rs src/newapi/managed_channels.rs
git diff --check a42545f
git diff --exit-code a42545f -- src/main.rs src/newapi.rs src/newapi/transport.rs src/newapi/managed_channels.rs src/orchestrator.rs src/quota.rs src/boot.rs src/status.rs src/model_catalog.rs src/config.rs src/catalog/schema_v1.sql src/catalog/schema_v2.sql src/catalog/schema_v3.sql src/catalog/store.rs src/catalog/types.rs src/catalog/legacy.rs Cargo.toml Cargo.lock
for pair in 'bind_created 14' 'confirm_member 15' 'defer_member 16'; do
  read -r name step <<< "$pair"
  sed -n "/fn $name(/,/^    }/p" src/catalog/confirmation.rs | rg "// Step P${step}:" >/dev/null
done
if rg 'reqwest|\.send\(|async fn|tokio' src/catalog/confirmation.rs; then
  printf 'Catalog confirmation must not perform network IO\n' >&2
  exit 1
fi
test "$(rg -l 'control_enum!\(OperationPhase' src | wc -l)" -eq 1
test "$(rg -l 'control_enum!\(ControlErrorCode' src | wc -l)" -eq 1
output="$(cargo test --locked --offline confirmation_operation_schema_output -- --nocapture)"
views="$(printf '%s\n' "$output" | sed -n 's/^CONFIRMATION_SCHEMA_JSON=//p')"
test -n "$views"
while IFS= read -r view; do
  check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/control-operation.schema.json --force-filetype json <(printf '%s\n' "$view")
done < <(printf '%s\n' "$views" | jq -c '.[]')
output="$(cargo test --locked --offline catalog_schema_output -- --nocapture)"
snapshot="$(printf '%s\n' "$output" | sed -n 's/^CATALOG_SCHEMA_JSON=//p')"
check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/catalog-snapshot.schema.json --force-filetype json <(printf '%s\n' "$snapshot")
printf 'SGM-2 binding/readback checks passed; HTTP worker remains pending\n'
