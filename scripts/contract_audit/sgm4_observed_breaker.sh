#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=1872ab3
git diff --check "$base"
while IFS= read -r path; do
  case "$path" in
    src/ledger/mod.rs|src/ledger/decode.rs|src/ledger/observations.rs|src/ledger/observations/*|src/source_policy/volcano.rs|src/source_policy/volcano/tests.rs|src/catalog/policy_tests.rs|src/catalog/policy_migration_tests.rs|docs/design/source-groups-token-management/*|docs/research/source-groups-facts-capability-research.md|docs/audits/sgm-4-facts-analytics/observed-breaker*|scripts/contract_audit/sgm4_observed_breaker.sh|scripts/tests/performance-gates.sh|scripts/tests/performance-gates-regression.sh|proptest-regressions/ledger/observations/*) ;;
    *) printf 'Unexpected observed breaker path: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <({ git diff --name-only "$base"; git ls-files --others --exclude-standard; } | LC_ALL=C sort -u)
git diff --exit-code "$base" -- Cargo.toml Cargo.lock src/catalog/schema_v*.sql src/source_policy/glm.rs src/orchestrator.rs src/newapi.rs src/newapi src/reconcile src/main.rs src/boot.rs .github
diff -u <(git show "$base:src/ledger/mod.rs") <(sed '/^pub(crate) mod observations;$/d' src/ledger/mod.rs)
diff -u <(git show "$base:src/ledger/decode.rs" | sed 's/^fn valid_request_id/pub(crate) fn valid_request_id/') src/ledger/decode.rs
rename_types() { perl -pe 's/\bFinalOutcome\b/OutcomeObservation/g; s/\bcompleted_at\b/observed_at/g'; }
for path in src/source_policy/volcano/tests.rs src/catalog/policy_tests.rs src/catalog/policy_migration_tests.rs; do
  diff -u <(git show "$base:$path" | rename_types | rustfmt --edition 2021 --config skip_children=true --emit stdout) "$path"
done
# 除类型名称、时间字段名称及注释，原状态机算法不能改变。
diff -u <(git show "$base:src/source_policy/volcano.rs" | rename_types | sed '/^[[:space:]]*\/\//d' | rustfmt --edition 2021 --config skip_children=true --emit stdout) <(sed '/^[[:space:]]*\/\//d' src/source_policy/volcano.rs | rustfmt --edition 2021 --config skip_children=true --emit stdout)
test "$(rg -F -c '// Step O1:' src/ledger/observations.rs)" -eq 1
test "$(rg -F -c '// Step O2:' src/ledger/observations.rs)" -eq 1
test "$(rg -F -c '// Step O3:' src/source_policy/volcano.rs)" -eq 1
if rg 'FinalOutcome|completed_at' src/source_policy/volcano.rs src/source_policy/volcano/tests.rs src/catalog/policy_tests.rs src/catalog/policy_migration_tests.rs; then exit 1; fi
if rg 'reqwest|rusqlite|std::fs|std::time|tokio|pub.*enum OutcomeKind|pub.*struct VolcanoRuntime' src/ledger/observations.rs; then exit 1; fi
rustfmt --edition 2021 --config skip_children=true --check src/ledger/mod.rs src/ledger/decode.rs src/ledger/observations.rs src/ledger/observations/tests.rs src/source_policy/volcano.rs src/source_policy/volcano/tests.rs src/catalog/policy_tests.rs src/catalog/policy_migration_tests.rs
bash scripts/tests/performance-gates-regression.sh
expected="$(printf 'ledger::observations::tests::%s\n' \
  observed_breaker_success_tokens_include_nonstream_done_eof_and_client_gone \
  observed_breaker_unknown_zero_and_explicit_failures_never_look_healthy \
  observed_breaker_errors_use_upstream_whitelist_and_exclusions_first \
  observed_breaker_failure_failure_success_failure_resets_without_unlatching \
  observed_breaker_shared_source_dedup_and_other_source_success_are_isolated \
  observed_breaker_identity_validation_and_failed_inputs_are_atomic \
  observed_breaker_late_success_respects_existing_cutoff_and_period \
  observed_breaker_private_request_material_has_no_logging_or_wire_traits \
  observed_pb1_classification_matches_independent_table \
  observed_pb2_request_source_replays_and_reordering_are_idempotent \
  observed_pb3_token_activity_and_source_counters_match_independent_model \
  performance_gate_observed_breaker_100k | LC_ALL=C sort)"
expected_ignored=ledger::observations::tests::performance_gate_observed_breaker_100k
manifest_ok() { [[ -n "$1" && "$1" == "$expected" && "$2" == "$expected_ignored" && "$1" != "$2" ]]; }
if manifest_ok '' ''; then exit 1; fi
if manifest_ok "$expected" "$expected"; then exit 1; fi
if manifest_ok "${expected#*$'\n'}" "$expected_ignored"; then exit 1; fi
listing="$(cargo test --locked --offline -j 2 -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^ledger::observations::' | LC_ALL=C sort || true)"
listing="$(cargo test --locked --offline -j 2 -- --ignored --list)"
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^ledger::observations::' | LC_ALL=C sort || true)"
if ! manifest_ok "$actual" "$ignored"; then printf 'Observed breaker test manifest mismatch\n' >&2; exit 1; fi
cargo test --locked --offline -j 2 ledger::observations::
printf 'Observed breaker scope, unchanged legacy assertions/algorithm, and 12-entry manifest passed. Full regression, mutations and standalone performance remain separate.\n'
