#!/usr/bin/env bash
set -euo pipefail
rustfmt --edition 2021 --check src/catalog/mod.rs src/reconcile/mod.rs src/newapi/managed_channels.rs src/newapi/transport.rs
git diff --check 4c35b78
git diff --exit-code 4c35b78 -- src/main.rs src/orchestrator.rs src/quota.rs src/boot.rs src/status.rs src/model_catalog.rs src/config.rs src/catalog/schema_v1.sql src/catalog/schema_v2.sql src/catalog/schema_v3.sql src/catalog/legacy.rs Cargo.toml Cargo.lock
for pair in 'mutate 13' 'write 14' 'round 16'; do
  read -r name step <<< "$pair"
  sed -n "/fn $name[<(]/,/^    }/p" src/reconcile/worker.rs | rg "// Step P${step}:" >/dev/null
done
gate="$(sed -n '/fn gate(/,/^    }/p' src/reconcile/worker.rs)"
test -n "$gate"
if printf '%s\n' "$gate" | rg 'core|store|lock\(|await|load_work'; then
  printf 'Admission guard must not lock or perform IO\n' >&2
  exit 1
fi
blocker="$(sed -n '/fn actionable_blocker(/,/^    }/p' src/reconcile/worker.rs)"
test -n "$blocker"
if printf '%s\n' "$blocker" | rg 'assemble\(|\.store|load_group_work'; then
  printf 'Blocker diagnostic must reuse the group snapshot\n' >&2
  exit 1
fi
observed="$(sed -n '/fn record_observed(/,/^    }/p' src/catalog/confirmation.rs)"
printf '%s\n' "$observed" | rg 'w.revision\(\) == u.revision' >/dev/null
if printf '%s\n' "$observed" | rg 'applied_revision|operation_targets|control_operations'; then
  printf 'Diagnostic observations cannot confirm operations\n' >&2
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
printf 'SGM-2 worker checkpoint checks passed; production preflight and full exit gates remain pending\n'
