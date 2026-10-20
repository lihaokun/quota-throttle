#!/usr/bin/env bash
set -euo pipefail

base=e0cfb82
git diff --check "$base"
# 本小步只新增测试入口和迁移组合测试，生产代码/旧 DDL/依赖零变更。
git diff --exit-code "$base" -- src Cargo.toml Cargo.lock \
  ':(exclude)src/catalog/policy_tests.rs' ':(exclude)src/catalog/policy_migration_tests.rs'
while IFS= read -r path; do
  case "$path" in
    src/catalog/policy_migration_tests.rs|scripts/contract_audit/sgm3_b_final.sh|docs/audits/sgm-3-source-policies/*|proptest-regressions/catalog/policy_migration_tests.txt) ;;
    *) printf 'Unexpected untracked file: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <(git ls-files --others --exclude-standard)
for step in 1 2 3 4 5 6 7; do
  test "$(rg -c "// Step B$step:" src/source_policy/volcano.rs)" -eq 1
done
test "$(rg -c '// Step B8:' src/catalog/store.rs)" -eq 1
for step in 9 10; do
  test "$(rg -c "// Step B$step:" src/catalog/policy.rs)" -eq 1
done
if rg 'enum (SourceKind|MembershipLifecycle|Regime)' src/catalog/policy.rs src/source_policy/volcano.rs; then
  printf 'Shared enums must not be redefined\n' >&2; exit 1
fi
rustfmt --edition 2021 --check src/catalog/policy_tests.rs src/source_policy/volcano.rs src/catalog/policy.rs

# 复用既有输出 fixture，核公开 wire 仍符合 v1，不改历史审核脚本基线。
schema_dir=docs/design/source-groups-token-management/schemas
check-jsonschema --check-metaschema "$schema_dir/catalog-snapshot.schema.json"
check-jsonschema --check-metaschema "$schema_dir/control-operation.schema.json"
catalog_output="$(cargo test --locked --offline -j 2 catalog_schema_output -- --nocapture)"
snapshot="$(printf '%s\n' "$catalog_output" | sed -n 's/^CATALOG_SCHEMA_JSON=//p')"
test -n "$snapshot"
check-jsonschema --schemafile "$schema_dir/catalog-snapshot.schema.json" --force-filetype json <(printf '%s\n' "$snapshot")
operation_output="$(cargo test --locked --offline -j 2 confirmation_operation_schema_output -- --nocapture)"
views="$(printf '%s\n' "$operation_output" | sed -n 's/^CONFIRMATION_SCHEMA_JSON=//p')"
test -n "$views"
while IFS= read -r view; do
  check-jsonschema --schemafile "$schema_dir/control-operation.schema.json" --force-filetype json <(printf '%s\n' "$view")
done < <(printf '%s\n' "$views" | jq -c '.[]')
cargo test --locked --offline -j 2 source_policy::volcano -- --test-threads=2
cargo test --locked --offline -j 2 catalog::store::tests::policy -- --test-threads=2
printf 'SGM-3 B final mechanical checks passed; deployment and C/SGM-4 remain separate\n'
