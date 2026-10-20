#!/usr/bin/env bash
set -euo pipefail
base=791bc56
git diff --check "$base"
git diff --exit-code "$base" -- src Cargo.toml Cargo.lock \
  ':(exclude)src/catalog/glm_publication.rs' ':(exclude)src/catalog/glm_publication_tests.rs' \
  ':(exclude)src/catalog/glm_round.rs' ':(exclude)src/catalog/glm_round_tests.rs' \
  ':(exclude)src/catalog/mod.rs' ':(exclude)src/catalog/execution_tests.rs' \
  ':(exclude)src/reconcile/planner.rs' ':(exclude)src/reconcile/planner/tests.rs' \
  ':(exclude)src/reconcile/worker.rs' ':(exclude)src/reconcile/worker/policy.rs' \
  ':(exclude)src/reconcile/worker/policy_tests.rs' ':(exclude)src/reconcile/worker/tests.rs' \
  ':(exclude)src/reconcile/worker/scenarios.rs' ':(exclude)src/newapi/managed_channels/tests.rs'
while IFS= read -r path; do
  case "$path" in
    src/catalog/glm_publication.rs|src/catalog/glm_publication_tests.rs|src/reconcile/worker/policy.rs|src/reconcile/worker/policy_tests.rs|scripts/contract_audit/sgm3_c1_publication.sh|docs/audits/sgm-3-source-policies/*|proptest-regressions/*) ;;
    *) printf 'Unexpected untracked file: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <(git ls-files --others --exclude-standard)
test "$(rg -c '// Step C2:' src/reconcile/worker/policy.rs)" -eq 1
test "$(rg -c '// Step C5:' src/catalog/glm_publication.rs)" -eq 1
for step in 3 4; do
  test "$(rg -c "// Step C$step:" src/catalog/glm_round.rs)" -eq 1
done
if rg 'reqwest|\.await|SystemTime|std::fs|enum (SourceKind|Regime|StatusDirective)' src/catalog/glm_publication.rs src/reconcile/worker/policy.rs; then
  printf 'Publication slice must not own network, clock, or duplicate enums\n' >&2; exit 1
fi
rg -U '#\[cfg\(test\)\]\n    pub fn publish_route' src/reconcile/worker.rs >/dev/null
test "$(rg -c 'self.policy_config.dry_run' src/reconcile/worker.rs)" -eq 2
rg 'self.membership_id == snapshot.member.membership_id' src/reconcile/planner.rs >/dev/null
rg 'self.member_revision == snapshot.target.revision' src/reconcile/planner.rs >/dev/null
rustfmt --edition 2021 --check src/catalog/glm_publication.rs src/catalog/glm_publication_tests.rs src/catalog/glm_round.rs src/catalog/glm_round_tests.rs src/reconcile/worker.rs src/reconcile/planner.rs
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
cargo test --locked --offline -j 2 reconcile::worker::tests::policy_tests -- --test-threads=2 --nocapture
printf 'C-1 publication checks passed; shared acquisition and full C-1 acceptance remain separate\n'
