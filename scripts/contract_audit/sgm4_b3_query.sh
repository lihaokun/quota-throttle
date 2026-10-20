#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=603f83f
git diff --check "$base"
git diff --exit-code "$base" -- src Cargo.toml Cargo.lock .github \
  ':(exclude)src/ledger/mod.rs' ':(exclude)src/ledger/analytics.rs' ':(exclude)src/ledger/analytics/'
diff -u <(git show "$base:src/ledger/mod.rs") <(sed '/^pub(crate) mod analytics;$/d' src/ledger/mod.rs)
while IFS= read -r path; do
  case "$path" in
    src/ledger/analytics.rs|src/ledger/analytics/*|proptest-regressions/ledger/analytics/*|scripts/contract_audit/sgm4_b3_query.sh|docs/audits/sgm-4-facts-analytics/*) ;;
    *) printf 'Unexpected untracked file: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <(git ls-files --others --exclude-standard)
test "$(rg -c '// Step B3.1:' src/ledger/analytics/query.rs)" -eq 1
test "$(rg -c '// Step B3.2:' src/ledger/analytics.rs)" -eq 1
if rg 'FinalOutcome|VolcanoEvidence|RoutingPermit|Connection|reqwest|std::fs|enum (AttributionIssue|ScanPhase)|const JSON_SAFE' src/ledger/analytics.rs src/ledger/analytics/query.rs; then exit 1; fi
rustfmt --edition 2021 --config skip_children=true --check src/ledger/analytics.rs src/ledger/analytics/query.rs src/ledger/analytics/tests.rs src/ledger/analytics/tests/properties.rs
bash -n scripts/tests/performance-gates.sh scripts/tests/performance-gates-regression.sh
bash scripts/tests/performance-gates-regression.sh
expected="$(printf '%s\n' \
  ledger::analytics::tests::query_rejects_invalid_domains_without_normalizing_labels_or_ids \
  ledger::analytics::tests::query_short_edges_and_local_bucket_boundaries_are_exact \
  ledger::analytics::tests::analytics_unknown_models_zero_consumption_and_stable_identity_are_preserved \
  ledger::analytics::tests::analytics_all_filters_are_anded_and_missing_ids_return_observed_empty \
  ledger::analytics::tests::analytics_empty_and_uninitialized_are_not_conflated \
  ledger::analytics::tests::analytics_errors_destroy_partial_work_and_cannot_be_ignored \
  ledger::analytics::tests::analytics_control_cancel_and_deadline_are_checked_before_publish \
  ledger::analytics::tests::analytics_model_and_token_capacity_include_unknown_and_reject_plus_one \
  ledger::analytics::tests::analytics_bucket_model_capacity_and_complete_output \
  ledger::analytics::tests::analytics_private_types_have_no_logging_or_wire_traits \
  ledger::analytics::tests::properties::pb1_query_partition_and_timezone_match_independent_reference \
  ledger::analytics::tests::properties::pb2_analytics_filters_and_all_partitions_match_raw_reference \
  ledger::analytics::tests::properties::analytics_pbt_minute_deltas_equal_individual_consumption \
  ledger::analytics::tests::properties::analytics_pbt_cancel_or_overflow_never_publishes_partial_result \
  ledger::analytics::tests::performance_gate_analytics_maximum_output | LC_ALL=C sort)"
listing="$(cargo test --locked --offline -j 2 -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^ledger::analytics::' | LC_ALL=C sort || true)"
if [[ "$actual" != "$expected" ]]; then printf 'B3 query test manifest mismatch\n' >&2; exit 1; fi
listing="$(cargo test --locked --offline -j 2 -- --ignored --list)"
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^ledger::analytics::' | LC_ALL=C sort || true)"
test "$ignored" = 'ledger::analytics::tests::performance_gate_analytics_maximum_output'
cargo test --locked --offline -j 2 ledger::analytics::
printf 'B3 query mechanical checks passed; full regression, mutation and performance evidence remain separate\n'
