#!/usr/bin/env bash
# B3.3–7 的新基线；不修改 B3.1–2 的历史范围门禁。
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=4aa89d6
git diff --check "$base"
allowed() {
  case "$1" in
    src/catalog/store.rs|src/catalog/ledger.rs|src/catalog/ledger/rows.rs|src/catalog/ledger/query.rs|src/catalog/ledger/attribution.rs|src/catalog/ledger_tests.rs|src/catalog/ledger_query_tests.rs|src/catalog/ledger_query_scale.rs|src/catalog/ledger_attribution_tests.rs|src/ledger/analytics.rs|src/ledger/analytics/tests.rs|src/ledger/analytics/tests/control_tests.rs|src/reconcile/worker.rs|src/reconcile/worker/analytics.rs|src/reconcile/worker/analytics/*|scripts/contract_audit/sgm4_b3_completion.sh|scripts/tests/performance-gates.sh|scripts/tests/performance-gates-regression.sh|docs/audits/sgm-4-facts-analytics/*|docs/design/source-groups-token-management/implementation-plan.md|docs/design/source-groups-token-management/subplans/sgm-4-facts-analytics-b3.md|proptest-regressions/catalog/ledger_attribution_tests.txt) return 0 ;;
    *) printf 'Unexpected B3 completion path: %s\n' "$1" >&2; return 1 ;;
  esac
}
while IFS= read -r path; do allowed "$path"; done < <({ git diff --name-only "$base"; git ls-files --others --exclude-standard; } | LC_ALL=C sort -u)
git diff --exit-code "$base" -- Cargo.toml Cargo.lock src/catalog/schema_v*.sql .github src/orchestrator.rs src/source_policy src/newapi src/main.rs src/boot.rs src/web
for pair in 'B3.3:src/catalog/ledger/query.rs' 'B3.4:src/reconcile/worker/analytics.rs' 'B3.5:src/catalog/ledger/attribution.rs' 'B3.6:src/catalog/ledger/attribution.rs' 'B3.7:src/reconcile/worker/analytics.rs'; do
  test "$(rg -F -c "// Step ${pair%%:*}:" "${pair#*:}")" -eq 1
done
if rg 'FinalOutcome|VolcanoEvidence|RoutingPermit|reqwest|enum (AttributionIssue|ScanPhase)|const JSON_SAFE' src/catalog/ledger/query.rs src/catalog/ledger/attribution.rs src/ledger/analytics.rs; then exit 1; fi
rustfmt --edition 2021 --config skip_children=true --check \
  src/catalog/store.rs src/catalog/ledger.rs src/catalog/ledger/rows.rs src/catalog/ledger/query.rs src/catalog/ledger/attribution.rs \
  src/catalog/ledger_tests.rs src/catalog/ledger_query_tests.rs src/catalog/ledger_query_scale.rs src/catalog/ledger_attribution_tests.rs \
  src/ledger/analytics.rs src/ledger/analytics/tests.rs src/ledger/analytics/tests/control_tests.rs \
  src/reconcile/worker.rs src/reconcile/worker/analytics.rs src/reconcile/worker/analytics/tests.rs src/reconcile/worker/analytics/tests/attribution_tests.rs
bash -n scripts/tests/performance-gates.sh scripts/tests/performance-gates-regression.sh
bash scripts/tests/performance-gates-regression.sh
prefixes='^(catalog::ledger::query::tests::|catalog::store::tests::ledger::(queries|attribution)::|ledger::analytics::tests::control_tests::|reconcile::worker::analytics::)'
expected="$({
  printf 'catalog::ledger::query::tests::%s\n' \
    query_connection_flags_are_readonly_nonwaiting_and_reject_writes query_connection_closes_before_last_directory_guard_unlocks
  printf 'catalog::store::tests::ledger::queries::%s\n' \
    query_snapshot_mixes_exact_edges_full_minutes_and_all_token_identities query_read_transaction_does_not_block_writer_or_mix_revisions \
    query_guard_outlives_store_without_reviving_duplicate_file_ownership query_budget_is_pre_filter_exact_and_cancelled_pages_never_publish \
    query_uninitialized_corrupt_and_changed_identity_are_explicit query_private_target_and_errors_do_not_expose_paths_or_labels \
    query_keyset_uses_indexes_without_rescanning_equal_time_prefixes query_pbt_sql_edges_filters_and_snapshot_match_raw_facts \
    query_pb6_concurrent_writes_never_mix_snapshot_revisions scale::performance_gate_analytics_sql_100k scale::analytics_sql_large_history_trends
  printf 'catalog::store::tests::ledger::attribution::%s\n' \
    attribution_moves_unknown_partitions_without_changing_native_facts_or_totals attribution_fixed_pass_replay_reopen_and_late_mapping_do_not_skip_unknown_history \
    attribution_empty_foreign_and_uninitialized_cursors_are_explicit attribution_sql_failure_and_late_overflow_roll_back_the_entire_batch \
    attribution_unknown_commit_poisoning_requires_reopen_before_retry attribution_corrupt_balances_and_rebinding_known_identity_are_rejected \
    attribution_readers_observe_complete_before_or_after_partitions attribution_private_cursor_and_batch_have_no_wire_or_logging_traits \
    attribution_max_id_and_deep_pages_seek_the_primary_key performance_gate_attribution_batch_10k_history \
    attribution_pb3_history_identity_and_counts_follow_independent_reference attribution_pb4_failure_replay_and_reopen_are_atomic \
    attribution_pb5_late_mapping_restarts_and_frozen_high_water_progress
  printf 'ledger::analytics::tests::control_tests::%s\n' \
    query_control_cancel_before_registration_and_registration_race_are_safe query_control_interrupts_actual_sql_after_registration
  printf 'reconcile::worker::analytics::tests::%s\n' \
    query_worker_reads_without_command_or_routing_locks_and_keeps_state query_worker_core_contention_is_cancellable_before_dispatch \
    query_worker_allows_control_writes_while_snapshot_is_open query_worker_cancel_keeps_slot_and_directory_guard_until_actual_exit \
    query_worker_timeout_does_not_release_running_slot_early query_worker_poisoned_writer_blocks_new_and_completed_results \
    query_worker_queued_cancellation_keeps_guard_and_never_runs_query
  printf 'reconcile::worker::analytics::tests::attribution_tests::%s\n' \
    attribution_worker_batches_and_replays_preserve_routing_and_totals attribution_worker_dry_run_busy_and_uninitialized_do_not_advance \
    attribution_worker_restart_revisits_earlier_unknown_facts attribution_worker_commit_unknown_does_not_publish_cursor_or_reopen_gate \
    attribution_worker_query_and_repair_share_one_atomic_ledger
} | LC_ALL=C sort)"
expected_ignored="$(printf '%s\n' \
  catalog::store::tests::ledger::queries::scale::performance_gate_analytics_sql_100k \
  catalog::store::tests::ledger::queries::scale::analytics_sql_large_history_trends \
  catalog::store::tests::ledger::attribution::performance_gate_attribution_batch_10k_history | LC_ALL=C sort)"
check_manifest() {
  [[ -n "$1" && "$1" == "$expected" && "$2" == "$expected_ignored" && "$1" != "$2" ]]
}
# 空清单、全 ignored 及漏项不能被“0 tests passed”蒙混。
if check_manifest '' ''; then exit 1; fi
if check_manifest "$expected" "$expected"; then exit 1; fi
if check_manifest "${expected#*$'\n'}" "$expected_ignored"; then exit 1; fi
listing="$(cargo test --locked --offline -j 2 -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg "$prefixes" | LC_ALL=C sort || true)"
listing="$(cargo test --locked --offline -j 2 -- --ignored --list)"
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg "$prefixes" | LC_ALL=C sort || true)"
if ! check_manifest "$actual" "$ignored"; then printf 'B3 completion test manifest mismatch\n' >&2; exit 1; fi
for prefix in catalog::ledger::query::tests:: catalog::store::tests::ledger::queries:: catalog::store::tests::ledger::attribution:: ledger::analytics::tests::control_tests:: reconcile::worker::analytics::; do
  cargo test --locked --offline -j 2 "$prefix"
done
printf 'B3 completion: exact 40-entry manifest (37 ordinary / 2 performance / 1 trend), scope and mechanical checks passed. Full regression, mutations, standalone performance and explicit trend execution remain separate.\n'
