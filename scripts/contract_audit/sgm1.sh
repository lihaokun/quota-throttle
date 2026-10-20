#!/usr/bin/env bash
# 在研究 worktree 根目录运行；只执行隔离测试和只读 diff 检查。
set -euo pipefail
audit_base=27a03b7
schema=docs/design/source-groups-token-management/schemas/catalog-snapshot.schema.json
command -v jq >/dev/null
command -v check-jsonschema >/dev/null
rustfmt --edition 2021 --check src/catalog/mod.rs
check-jsonschema --check-metaschema "$schema"

test_output="$(cargo test --locked --offline catalog::store::tests::catalog_schema_output -- --exact --nocapture)"
snapshot_json="$(printf '%s\n' "$test_output" | sed -n 's/^CATALOG_SCHEMA_JSON=//p')"
enum_json="$(printf '%s\n' "$test_output" | sed -n 's/^CATALOG_ENUM_JSON=//p')"
test -n "$snapshot_json"
test -n "$enum_json"
check-jsonschema --schemafile "$schema" --force-filetype json <(printf '%s\n' "$snapshot_json")
expected_enums="$(jq -c '{kind: ."$defs".kind.enum, lifecycle: ."$defs".membership.properties.lifecycle.enum}' "$schema")"
jq -en --argjson actual "$enum_json" --argjson expected "$expected_enums" '$actual == $expected' >/dev/null
kind_sql="$(printf '%s\n' "$enum_json" | jq -r '.kind | map("\u0027" + . + "\u0027") | join(", ")')"
lifecycle_sql="$(printf '%s\n' "$enum_json" | jq -r '.lifecycle | map("\u0027" + . + "\u0027") | join(", ")')"
test "$(rg -F -c "CHECK(kind IN ($kind_sql))" src/catalog/schema_v1.sql)" -eq 3
test "$(rg -F -c "CHECK(lifecycle IN ($lifecycle_sql))" src/catalog/schema_v1.sql)" -eq 1

# 验证拒绝路径，避免宽松 schema 给正常样本假阳性。
for filter in '.sources[0].credential="fake-secret"' '.groups[0].newapi_group="auto"' \
  '.memberships[0].channel_id=null' '.memberships[1].channel_id="42"' \
  '.memberships[0].lifecycle="retired"' '.sources[0].type="unknown"' \
  '.sources[0].source_id=1' '.sources[0].config_revision=0'; do
  if check-jsonschema --quiet --schemafile "$schema" --force-filetype json \
      <(printf '%s\n' "$snapshot_json" | jq "$filter") >/dev/null; then
    printf 'Schema unexpectedly accepted: %s\n' "$filter" >&2
    exit 1
  fi
done

for step in 1 2 3 4 5; do
  sed -n '/pub fn open(path:/,/^    }/p' src/catalog/store.rs | rg "// Step P${step}:" >/dev/null
done
for step in 6 7 8 9 10; do
  sed -n '/pub fn build_legacy_import(/,$p' src/catalog/legacy.rs | rg "// Step P${step}:" >/dev/null
done
for step in 11 12 13 14; do
  sed -n '/pub fn import_legacy(/,/^    }/p' src/catalog/store.rs | rg "// Step P${step}:" >/dev/null
done

# 一个枚举定义、跨模块只引用；模型发现鉴权继续复用原 config 类型。
test "$(rg -l 'wire_enum!\(SourceKind' src/catalog | wc -l)" -eq 1
test "$(rg -l 'wire_enum!\(MembershipLifecycle' src/catalog | wc -l)" -eq 1
test "$(rg -l 'enum ModelDiscoveryAuth' src | wc -l)" -eq 1
git diff --exit-code "$audit_base" -- src/orchestrator.rs src/quota.rs src/newapi.rs src/boot.rs src/status.rs src/model_catalog.rs
main_additions="$(git diff --unified=0 "$audit_base" -- src/main.rs | sed -n '/^+++ /d; s/^+//p')"
test "$main_additions" = 'pub mod catalog;'
test -z "$(git diff --unified=0 "$audit_base" -- src/main.rs | sed -n '/^--- /d; s/^-//p')"
config_changes="$(git diff --unified=0 "$audit_base" -- src/config.rs | sed '/^+++ /d; /^--- /d' | sed -n 's/^[+-]//p')"
if printf '%s\n' "$config_changes" | rg -v '^#\[derive\(.*\)\]$|^use serde::'; then
  printf 'Unexpected config logic change\n' >&2
  exit 1
fi
git diff --check "$audit_base"
printf 'sgm-1 mechanical contract checks passed\n'
