#!/usr/bin/env bash
set -euo pipefail
base=8fa37e1
git diff --check "$base"
git diff --exit-code "$base" -- src Cargo.toml Cargo.lock \
  ':(exclude)src/catalog/glm_publication.rs' ':(exclude)src/catalog/glm_publication_tests.rs' \
  ':(exclude)src/catalog/glm_round.rs' ':(exclude)src/catalog/glm_round_tests.rs' ':(exclude)src/catalog/store.rs' \
  ':(exclude)src/reconcile/worker.rs' ':(exclude)src/reconcile/worker/policy.rs' \
  ':(exclude)src/reconcile/worker/policy_tests.rs' ':(exclude)src/reconcile/worker/acquisition_tests.rs' \
  ':(exclude)src/source_policy/mod.rs'
diff -u <(git show "$base:src/catalog/store.rs" | sed 's/^fn read_material(/pub(super) fn read_material(/' | rustfmt --edition 2021 --config skip_children=true) src/catalog/store.rs
diff -u <(git show "$base:src/source_policy/mod.rs" | tail -n +2) <(tail -n +2 src/source_policy/mod.rs)
# 许可/恢复/提交包装保持不变；目标适配的回读优化另做逐字段对照。
diff -u <(git show "$base:src/catalog/glm_publication.rs" | sed -n '/^fn build_glm_permits(/,/^impl CatalogStore/p') <(sed -n '/^fn build_glm_permits(/,/^impl CatalogStore/p' src/catalog/glm_publication.rs)
diff -u <(git show "$base:src/catalog/glm_publication.rs" | sed -n '/^    pub(crate) fn preview_glm_round(/,$p') <(sed -n '/^    pub(crate) fn preview_glm_round(/,$p' src/catalog/glm_publication.rs)
while IFS= read -r path; do
  case "$path" in
    src/reconcile/worker/acquisition_tests.rs|scripts/contract_audit/sgm3_c1_acquisition.sh|docs/audits/sgm-3-source-policies/*|proptest-regressions/*) ;;
    *) printf 'Unexpected untracked file: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <(git ls-files --others --exclude-standard)
for step in 1 5; do test "$(rg -c "// Step C$step:" src/catalog/glm_publication.rs)" -eq 1; done
test "$(rg -c '// Step C2:' src/reconcile/worker/policy.rs)" -eq 1
for step in 3 4; do test "$(rg -c "// Step C$step:" src/catalog/glm_round.rs)" -eq 1; done
if rg 'reqwest|\.await|SystemTime|std::fs|enum (SourceKind|Regime|StatusDirective)' src/catalog/glm_publication.rs; then exit 1; fi
if rg 'tokio::spawn|reqwest|collect_cycle.*spawn' src/reconcile/worker/policy.rs; then exit 1; fi
rg -U '#\[cfg\(test\)\]\n    pub\(super\) fn publish_glm_cycle' src/reconcile/worker/policy.rs >/dev/null
rustfmt --edition 2021 --check src/catalog/glm_publication.rs src/catalog/glm_publication_tests.rs src/catalog/glm_round_tests.rs src/reconcile/worker.rs
output="$(cargo test --locked --offline -j 2 catalog_schema_output -- --nocapture)"
snapshot="$(printf '%s\n' "$output" | sed -n 's/^CATALOG_SCHEMA_JSON=//p')"; test -n "$snapshot"
check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/catalog-snapshot.schema.json --force-filetype json <(printf '%s\n' "$snapshot")
output="$(cargo test --locked --offline -j 2 confirmation_operation_schema_output -- --nocapture)"
views="$(printf '%s\n' "$output" | sed -n 's/^CONFIRMATION_SCHEMA_JSON=//p')"; test -n "$views"
while IFS= read -r view; do
  check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/control-operation.schema.json --force-filetype json <(printf '%s\n' "$view")
done < <(printf '%s\n' "$views" | jq -c '.[]')
cargo test --locked --offline -j 2 source_policy::tests -- --test-threads=2
cargo test --locked --offline -j 2 catalog::store::tests::glm_round -- --test-threads=2 --nocapture
cargo test --locked --offline -j 2 reconcile::worker::tests::policy_tests -- --test-threads=2 --nocapture
printf 'C-1 acquisition checks passed; full regression and complete C-1 audit remain separate\n'
