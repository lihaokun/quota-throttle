#!/usr/bin/env bash
# 第三批当前小步：纯计划 + 最终准入门禁；不把 P13–P16 持久执行记为完成。
set -euo pipefail
rustfmt --edition 2021 --check src/reconcile/mod.rs src/newapi.rs src/catalog/mod.rs
git diff --check c71a9fc
git diff --exit-code c71a9fc -- src/orchestrator.rs src/quota.rs src/boot.rs src/status.rs src/model_catalog.rs src/config.rs src/catalog/store.rs src/catalog/schema_v1.sql src/catalog/schema_v2.sql src/catalog/types.rs src/catalog/legacy.rs
main_changes="$(git diff --unified=0 c71a9fc -- src/main.rs | sed '/^+++ /d; /^--- /d' | sed -n '/^[+-]/p')"
test "$main_changes" = '+pub mod reconcile;'
for step in 11 12; do
  sed -n '/pub fn plan_next_action(/,/^}/p' src/reconcile/planner.rs | rg "// Step P${step}:" >/dev/null
done
for step in 2 3 13; do
  sed -n '/pub async fn send_fenced(/,/^    }/p' src/newapi/transport.rs | rg "// Step P${step}:" >/dev/null
done
if rg 'reqwest|rusqlite|\.send\(|\.execute\(' src/reconcile/planner.rs; then
  printf 'Planner is not pure\n' >&2
  exit 1
fi
if rg 'derive\([^)]*(Serialize|Deserialize)' src/reconcile; then
  printf 'Planner context must not become a frontend wire capability\n' >&2
  exit 1
fi
test "$(rg -l 'control_enum!\(StatusDirective' src | wc -l)" -eq 1
test "$(rg -l 'control_enum!\(CreateState' src | wc -l)" -eq 1
test "$(rg -l 'wire_enum!\(SourceKind' src | wc -l)" -eq 1

operation_output="$(cargo test --locked --offline control_operation_schema_output -- --nocapture)"
views="$(printf '%s\n' "$operation_output" | sed -n 's/^CONTROL_SCHEMA_JSON=//p')"
test -n "$views"
while IFS= read -r view; do
  check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/control-operation.schema.json \
    --force-filetype json <(printf '%s\n' "$view")
done < <(printf '%s\n' "$views" | jq -c '.[]')
snapshot_output="$(cargo test --locked --offline catalog_schema_output -- --nocapture)"
snapshot="$(printf '%s\n' "$snapshot_output" | sed -n 's/^CATALOG_SCHEMA_JSON=//p')"
check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/catalog-snapshot.schema.json \
  --force-filetype json <(printf '%s\n' "$snapshot")
printf 'sgm-2 planner/admission checks passed; persistent execution remains pending\n'
