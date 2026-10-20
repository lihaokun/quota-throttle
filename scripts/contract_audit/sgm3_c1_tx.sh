#!/usr/bin/env bash
set -euo pipefail
base=fce5187
git diff --check "$base"
git diff --exit-code "$base" -- src Cargo.toml Cargo.lock \
  ':(exclude)src/catalog/glm_round.rs' ':(exclude)src/catalog/glm_round_tests.rs' \
  ':(exclude)src/catalog/mod.rs' ':(exclude)src/catalog/tests.rs' \
  ':(exclude)src/catalog/store.rs' ':(exclude)src/catalog/execution.rs' \
  ':(exclude)src/catalog/policy.rs' ':(exclude)src/config.rs' ':(exclude)src/source_policy/probe.rs'
# 被复用代码只开放可见性，不允许混入旧算法/配置/存储语义变更。
diff -u <(git show "$base:src/catalog/store.rs" | sed 's/^fn read_snapshot(/pub(super) fn read_snapshot(/') src/catalog/store.rs
diff -u <(git show "$base:src/catalog/execution.rs" | sed 's/^fn group_work(/pub(super) fn group_work(/') src/catalog/execution.rs
diff -u <(git show "$base:src/catalog/policy.rs" | sed -E 's/^fn (group_record|read_group_policy)\(/pub(super) fn \1(/' | rustfmt --edition 2021) src/catalog/policy.rs
diff -u <(git show "$base:src/config.rs" | sed 's/^    fn validate(/    pub(crate) fn validate(/') src/config.rs
diff -u <(git show "$base:src/source_policy/probe.rs") \
  <(sed '/^    \/\/\/ 仅测试使用的原始输入/,/^    pub fn cycle_id/{ /^    pub fn cycle_id/!d; }' src/source_policy/probe.rs)
while IFS= read -r path; do
  case "$path" in
    src/catalog/glm_round.rs|src/catalog/glm_round_tests.rs|scripts/contract_audit/sgm3_c1_tx.sh|docs/audits/sgm-3-source-policies/*|proptest-regressions/catalog/glm_round_tests.txt) ;;
    *) printf 'Unexpected untracked file: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <(git ls-files --others --exclude-standard)
for step in 3 4; do
  test "$(rg -c "// Step C$step:" src/catalog/glm_round.rs)" -eq 1
done
if rg 'reqwest|\.await|SystemTime|std::fs|RoutingPermit|publish_route|enum (SourceKind|Regime|StatusDirective)' src/catalog/glm_round.rs; then
  printf 'Transaction slice must not own network, permit publication, or duplicate enums\n' >&2; exit 1
fi
rustfmt --edition 2021 --check src/catalog/glm_round.rs src/catalog/glm_round_tests.rs src/source_policy/probe.rs
output="$(cargo test --locked --offline -j 2 catalog_schema_output -- --nocapture)"
snapshot="$(printf '%s\n' "$output" | sed -n 's/^CATALOG_SCHEMA_JSON=//p')"
test -n "$snapshot"
check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/catalog-snapshot.schema.json --force-filetype json <(printf '%s\n' "$snapshot")
output="$(cargo test --locked --offline -j 2 confirmation_operation_schema_output -- --nocapture)"
views="$(printf '%s\n' "$output" | sed -n 's/^CONFIRMATION_SCHEMA_JSON=//p')"
test -n "$views"
while IFS= read -r view; do
  check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/control-operation.schema.json --force-filetype json <(printf '%s\n' "$view")
done < <(printf '%s\n' "$views" | jq -c '.[]')
cargo test --locked --offline -j 2 catalog::store::tests::glm_round -- --test-threads=2 --nocapture
printf 'C-1 transaction checks passed; worker, permits, acquisition and full C-1 acceptance remain separate\n'
