#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=ce23dc7
git diff --check "$base"
while IFS= read -r path; do
  case "$path" in
    src/catalog/control_tests.rs|src/catalog/execution_tests.rs|src/catalog/glm_publication.rs|src/catalog/ledger.rs|src/catalog/ledger/attribution.rs|src/catalog/ledger/observed.rs|src/catalog/ledger/observed/*|src/catalog/ledger_migration_tests.rs|src/catalog/ledger_query_tests.rs|src/catalog/mod.rs|src/catalog/policy.rs|src/catalog/policy_tests.rs|src/catalog/store.rs|src/catalog/tests.rs|src/catalog/types.rs|src/catalog/observed_publication*|src/catalog/schema_v6.sql|src/catalog/volcano_publication.rs|src/ledger/analytics/query.rs|src/ledger/mod.rs|src/reconcile/worker/policy.rs|src/reconcile/worker/scenarios.rs|src/reconcile/worker/tests.rs|src/reconcile/worker/volcano_tests.rs|docs/design/source-groups-token-management/*|docs/audits/sgm-4-facts-analytics/observed-publication*|scripts/contract_audit/sgm4_observed_publication.sh|scripts/tests/performance-gates.sh|scripts/tests/performance-gates-regression.sh) ;;
    *) printf 'Unexpected O4 path: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <({ git diff --name-only "$base"; git ls-files --others --exclude-standard; } | LC_ALL=C sort -u)
git diff --exit-code "$base" -- Cargo.toml Cargo.lock src/catalog/schema_v1.sql src/catalog/schema_v2.sql src/catalog/schema_v3.sql src/catalog/schema_v4.sql src/catalog/schema_v5.sql src/source_policy src/orchestrator.rs src/newapi.rs src/newapi src/main.rs src/boot.rs src/config.rs src/status.rs .github
for path in src/catalog/control_tests.rs src/catalog/execution_tests.rs src/catalog/policy_tests.rs; do
  diff -u <(git show "$base:$path" | sed -e 's/store.conn.execute_batch(DROP_LEDGER_V5).unwrap();/drop_ledger_for_legacy(\&store.conn);/' -e 's/tx.execute_batch(DROP_LEDGER_V5).unwrap();/drop_ledger_for_legacy(\&tx);/' | rustfmt --edition 2021 --config skip_children=true --emit stdout) "$path"
done
diff -u <(git show "$base:src/reconcile/worker/scenarios.rs" | sed 's/^async fn drive/pub(super) async fn drive/') src/reconcile/worker/scenarios.rs
diff -u <(git show "$base:src/ledger/mod.rs") <(sed '/^pub(crate) use decode::valid_request_id;$/d' src/ledger/mod.rs)
for step in O4.0 O4.1 O4.2 O4.3 O4.4 O4.5; do
  test "$(rg -F "// Step $step:" src/catalog/store.rs src/catalog/ledger/observed.rs src/catalog/ledger/observed/usage.rs src/catalog/volcano_publication.rs src/reconcile/worker/policy.rs | wc -l)" -eq 1
done
test "$(rg -c '^CREATE TABLE ' src/catalog/schema_v6.sql)" -eq 4
test "$(rg -c '^CREATE INDEX ' src/catalog/schema_v6.sql)" -eq 4
rg -q '^pub const DATABASE_SCHEMA_VERSION: i64 = 6;' src/catalog/types.rs
rg -q 'ON CONFLICT\(native_log_id\) DO NOTHING' src/catalog/ledger/observed.rs
if rg 'INSERT OR IGNORE|reqwest|tokio::spawn|pub .*Serialize' src/catalog/ledger/observed.rs src/catalog/volcano_publication.rs; then exit 1; fi
bash scripts/tests/performance-gates-regression.sh
expected="$(printf '%s\n' \
  catalog::ledger::observed::usage::tests::observed_usage_exact_edges_budgets_and_overflow_fail_closed \
  catalog::store::tests::observed_publication::observed_publication_persistent_success_and_retries_do_not_recount_after_reopen \
  catalog::store::tests::observed_publication::observed_publication_shared_source_exhausts_both_groups_without_changing_other_selection \
  catalog::store::tests::observed_publication::observed_publication_historical_usage_zero_tokens_and_boundaries_are_exact \
  catalog::store::tests::observed_publication::observed_publication_late_source_binding_enqueues_once \
  catalog::store::tests::observed_publication::observed_publication_old_queued_and_unqueued_failures_fence_later_success \
  catalog::store::tests::observed_publication::observed_publication_reset_history_and_old_request_isolation_are_atomic \
  catalog::store::tests::observed_publication::observed_publication_batches_stale_data_and_preview_do_not_grant_permission \
  catalog::store::tests::observed_publication::observed_publication_faults_roll_back_every_semantic_table \
  catalog::store::tests::observed_publication::observed_publication_unknown_commit_requires_reopen_and_replays_once \
  catalog::store::tests::observed_publication::observed_publication_sequence_exhaustion_rolls_back_fact_page \
  catalog::store::tests::observed_publication::observed_publication_future_queue_and_ineligible_members_never_authorize \
  catalog::store::tests::observed_publication::observed_publication_unsupported_upstream_is_not_queued_or_enabled \
  catalog::store::tests::observed_publication::observed_publication_group_only_attribution_does_not_reenqueue \
  catalog::store::tests::observed_publication::observed_publication_second_group_failure_rolls_back_prior_group_targets \
  catalog::store::tests::observed_publication::properties::observed_publication_pb1_v5_migration_preserves_state_and_does_not_replay \
  catalog::store::tests::observed_publication::properties::observed_publication_pb2_events_retries_reopens_match_independent_reference \
  catalog::store::tests::observed_publication::properties::observed_publication_pb3_fault_rollback_then_replay_is_once \
  catalog::store::tests::observed_publication::properties::observed_publication_v6_migration_collision_rolls_back_and_preserves_v5 \
  catalog::store::tests::observed_publication::properties::observed_publication_v6_corruption_is_rejected_without_repair \
  catalog::store::tests::observed_publication::scale::performance_gate_observed_period_usage_100k \
  catalog::store::tests::observed_publication::scale::performance_gate_observed_maximum_catalog \
  reconcile::worker::tests::volcano_tests::observed_worker_real_reconcile_disables_shared_source_before_promotion \
  reconcile::worker::tests::volcano_tests::observed_worker_glm_and_volcano_scopes_keep_other_provider_leases \
  reconcile::worker::tests::volcano_tests::observed_worker_failure_and_unknown_commit_do_not_publish_partial_state \
  reconcile::worker::tests::volcano_tests::observed_worker_dry_run_queue_full_and_uninitialized_do_not_authorize \
  reconcile::worker::tests::volcano_tests::observed_worker_retired_members_are_removed_only_by_their_provider \
  reconcile::worker::tests::volcano_tests::observed_worker_pb4_provider_publications_preserve_other_scope | LC_ALL=C sort)"
expected_ignored="$(printf '%s\n' "$expected" | rg '::performance_gate_')"
manifest_ok() { [[ -n "$1" && "$1" == "$expected" && "$2" == "$expected_ignored" && "$1" != "$2" ]]; }
if manifest_ok '' '' || manifest_ok "$expected" "$expected" || manifest_ok "${expected#*$'\n'}" "$expected_ignored"; then exit 1; fi
listing="$(cargo test --locked --offline -j 2 -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^(catalog::store::tests::observed_publication::|catalog::ledger::observed::usage::tests::|reconcile::worker::tests::volcano_tests::)' | LC_ALL=C sort || true)"
listing="$(cargo test --locked --offline -j 2 -- --ignored --list)"
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^(catalog::store::tests::observed_publication::|catalog::ledger::observed::usage::tests::|reconcile::worker::tests::volcano_tests::)' | LC_ALL=C sort || true)"
if ! manifest_ok "$actual" "$ignored"; then printf 'O4 test manifest mismatch\n' >&2; exit 1; fi
printf 'O4 scope, frozen algorithms/DDL, Step/schema markers, 28-entry test manifest and negative gates passed. Full regression/mutations/performance are separate.\n'
