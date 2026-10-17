#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=812b190
git diff --check "$base"
git diff --exit-code "$base" -- Cargo.toml Cargo.lock src \
  ':(exclude)src/ledger/mod.rs' ':(exclude)src/ledger/collection.rs' ':(exclude)src/ledger/collection/' \
  ':(exclude)src/catalog/ledger.rs' ':(exclude)src/catalog/ledger_tests.rs' ':(exclude)src/catalog/ledger_scan_tests.rs' \
  ':(exclude)src/newapi.rs' ':(exclude)src/newapi/logs.rs' ':(exclude)src/newapi/logs/' \
  ':(exclude)src/newapi/preflight.rs' ':(exclude)src/newapi/managed_channels/tests.rs' ':(exclude)src/newapi/transport/tests.rs' \
  ':(exclude)src/reconcile/worker.rs' ':(exclude)src/reconcile/worker/collection.rs' ':(exclude)src/reconcile/worker/collection/'
for path in src/catalog/schema_v{1,2,3,4,5}.sql; do git diff --exit-code "$base" -- "$path"; done
diff -u <(git show "$base:src/newapi.rs") <(sed '/^pub(crate) mod logs;$/d' src/newapi.rs)
diff -u <(git show "$base:src/ledger/mod.rs") <(sed '/^pub(crate) mod collection;$/d' src/ledger/mod.rs)
diff -u <(git show "$base:src/reconcile/worker.rs") <(sed '/^mod collection;$/d; /    collector: /d; /    collection_running: /d' src/reconcile/worker.rs)
diff -u <(git show "$base:src/catalog/ledger.rs") <(sed '/^    pub(crate) fn restart_scan/,/^    pub(crate) fn load_scan_ticket/{ /^    pub(crate) fn load_scan_ticket/!d; }' src/catalog/ledger.rs)
while IFS= read -r path; do
  case "$path" in
    src/ledger/collection.rs|src/ledger/collection/*|src/catalog/ledger_scan_tests.rs|src/newapi/logs.rs|src/newapi/logs/*|src/reconcile/worker/collection.rs|src/reconcile/worker/collection/*|scripts/probes/newapi_paging_probe.py|scripts/contract_audit/sgm4_b2.sh|docs/audits/sgm-4-facts-analytics/*|proptest-regressions/ledger/*|proptest-regressions/reconcile/worker/collection/*) ;;
    *) printf 'Unexpected untracked file: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <(git ls-files --others --exclude-standard)
for step in 1 2 3 4 5 6 7; do
  test "$(rg -c "// Step B2.$step:" src/newapi/logs.rs src/newapi/logs/capability.rs src/ledger/collection.rs src/catalog/ledger.rs src/reconcile/worker/collection.rs | awk -F: '{s+=$NF} END{print s}')" -eq 1
done
if rg 'FinalOutcome|VolcanoEvidence|RoutingPermit|enum (ScanLane|ScanPhase|ScanFault|PageEnd)|const JSON_SAFE' src/ledger/collection.rs src/newapi/logs.rs src/newapi/logs/capability.rs src/reconcile/worker/collection.rs; then exit 1; fi
rustfmt --edition 2021 --config skip_children=true --check src/catalog/ledger.rs src/catalog/ledger_scan_tests.rs src/ledger/collection.rs src/ledger/collection/tests.rs src/newapi/logs.rs src/newapi/logs/tests.rs src/newapi/logs/capability.rs src/newapi/logs/capability/tests.rs src/newapi/logs/capability/producer_tests.rs src/reconcile/worker/collection.rs src/reconcile/worker/collection/tests.rs src/reconcile/worker/collection/tests/http_tests.rs
bash -n scripts/tests/performance-gates.sh scripts/tests/performance-gates-regression.sh
bash scripts/tests/performance-gates-regression.sh
expected="$(printf '%s\n' \
  newapi::logs::tests::log_page_private_payload_has_no_debug_or_serde \
  newapi::logs::tests::log_page_query_bounds_and_movement_are_explicit \
  newapi::logs::tests::log_pages_validate_every_raw_row_and_do_not_trust_total \
  newapi::logs::tests::pb1_log_page_raw_count_and_endpoint_contract \
  newapi::logs::tests::log_read_reuses_authentication_and_only_reads_global_pages \
  newapi::logs::tests::log_read_rejects_control_class_and_preserves_429_even_with_oversize_body \
  newapi::logs::tests::log_read_401_replay_rechecks_cancellation \
  newapi::logs::tests::log_read_expired_lease_is_checked_after_budget_wait \
  newapi::logs::capability::tests::log_capability_startup_evidence_is_bounded_exact_and_private \
  newapi::logs::capability::tests::log_capability_lease_cannot_survive_monotonic_expiry_or_backwards_wall_clock \
  newapi::logs::capability::tests::log_capability_missing_local_binding_performs_no_management_io \
  newapi::logs::capability::tests::pb6_log_startup_evidence_never_defaults_missing_or_ambiguous_to_sqlite \
  newapi::logs::capability::producer_tests::log_capability_real_producer_rejects_missing_environment_and_mid_read_pid_change \
  newapi::logs::capability::producer_tests::log_capability_total_deadline_cancels_budget_wait_before_status_send \
  ledger::collection::tests::plan_preserves_fixed_windows_and_global_faults \
  ledger::collection::tests::pb2_plan_time_restart_and_persistent_history \
  catalog::store::tests::ledger::scans::restart_scan_preserves_facts_and_rejects_old_foreign_or_finished_tickets \
  catalog::store::tests::ledger::scans::restart_scan_failure_and_unknown_commit_are_atomic \
  catalog::store::tests::ledger::scans::pb3_restart_reopen_keeps_atomic_deduplicated_facts \
  catalog::store::tests::ledger::scans::collection_maximum_page_capacity \
  catalog::store::tests::ledger::scans::performance_gate_collection_page \
  catalog::store::tests::ledger::scans::performance_gate_collection_restart_100k \
  reconcile::worker::collection::tests::collection_interleaves_fixed_windows_and_deduplicates_across_lanes \
  reconcile::worker::collection::tests::collection_post_binding_check_discards_page_and_clock_failure_never_forges_time \
  reconcile::worker::collection::tests::collection_conflict_stops_both_lanes_and_bad_pages_never_skip \
  reconcile::worker::collection::tests::collection_dry_run_queue_capacity_and_429_do_not_add_writes_or_retry_loops \
  reconcile::worker::collection::tests::collection_total_deadline_and_shutdown_release_resources_without_losing_confirmed_state \
  reconcile::worker::collection::tests::pb4_collection_moving_offsets_and_late_history_match_observed_reference \
  reconcile::worker::collection::tests::pb5_collection_singleflight_cancellation_and_fair_retry \
  reconcile::worker::collection::tests::collection_background_sleep_stop_restart_and_multiple_runners_preserve_progress \
  reconcile::worker::collection::tests::collection_background_inflight_shutdown_and_dry_run_wait_without_spinning \
  reconcile::worker::collection::tests::collection_page_timeouts_retry_same_window_and_explicit_retry_does_not_skip \
  reconcile::worker::collection::tests::collection_waiting_page_does_not_hold_core_or_fence_unrelated_control \
  reconcile::worker::collection::tests::collection_unknown_commit_keeps_writer_poisoned_and_requires_reopen \
  reconcile::worker::collection::tests::collection_unencodable_saved_window_stalls_only_its_lane \
  reconcile::worker::collection::tests::http_tests::collection_http_401_physical_request_counts_and_logical_four_page_budget \
  reconcile::worker::collection::tests::http_tests::collection_http_background_read_does_not_occupy_control_slot \
  reconcile::worker::collection::tests::http_tests::collection_http_429_ends_round_even_when_body_is_not_json \
  reconcile::worker::collection::tests::http_tests::collection_http_count_find_changes_and_late_rows_converge_without_complete_claim \
  reconcile::worker::collection::tests::http_tests::collection_http_truncated_body_never_commits_partial_page | LC_ALL=C sort)"
pattern='^(newapi::logs::|ledger::collection::|catalog::store::tests::ledger::scans::|reconcile::worker::collection::)'
listing="$(cargo test --locked --offline -j 2 -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg "$pattern" | LC_ALL=C sort || true)"
if [[ "$actual" != "$expected" ]]; then printf 'B2 test manifest mismatch\n' >&2; exit 1; fi
listing="$(cargo test --locked --offline -j 2 -- --ignored --list)"
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg "$pattern" | LC_ALL=C sort || true)"
test "$ignored" = "$(printf '%s\n' "$expected" | rg '::performance_gate_')"
for filter in newapi::logs:: collection:: ledger::scans::; do cargo test --locked --offline -j 2 "$filter"; done
printf 'SGM-4 B2 mechanical checks passed; full regression, native, mutation and performance evidence remain separate\n'
