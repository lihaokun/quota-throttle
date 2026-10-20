#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=da35d8e
git diff --check "$base"
while IFS= read -r path; do
  case "$path" in
    src/catalog/control.rs|src/catalog/mod.rs|src/catalog/store.rs|src/catalog/types.rs|src/catalog/tests.rs|src/catalog/observed_publication_properties.rs|src/catalog/policy_command*|src/catalog/schema_v7.sql|src/model_catalog.rs|src/reconcile/worker.rs|src/reconcile/worker/tests.rs|src/reconcile/worker/volcano_tests.rs|src/reconcile/worker/management*|docs/design/source-groups-token-management/*|docs/audits/sgm-3-source-policies/c2-*|scripts/contract_audit/sgm3_c2.sh|scripts/tests/performance-gates.sh|scripts/tests/performance-gates-regression.sh|proptest-regressions/catalog/policy_command*|proptest-regressions/reconcile/worker/management*) ;;
    *) printf 'Unexpected C2 path: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <({ git diff --name-only "$base"; git ls-files --others --exclude-standard; } | LC_ALL=C sort -u)
git diff --exit-code "$base" -- Cargo.toml Cargo.lock src/catalog/schema_v1.sql src/catalog/schema_v2.sql src/catalog/schema_v3.sql src/catalog/schema_v4.sql src/catalog/schema_v5.sql src/catalog/schema_v6.sql src/source_policy src/ledger src/newapi src/newapi.rs src/main.rs src/boot.rs src/config.rs src/status.rs src/orchestrator.rs src/reconcile/planner.rs src/catalog/policy.rs src/catalog/confirmation.rs src/catalog/execution.rs
diff -u <(git show "$base:src/reconcile/worker/volcano_tests.rs" | sed 's/^fn add_glm/pub(super) fn add_glm/') src/reconcile/worker/volcano_tests.rs
for step in C2.0 C2.1 C2.2 C2.3 C2.4; do
  test "$(rg -F "// Step $step:" src/catalog/store.rs src/catalog/control.rs src/catalog/policy_commands.rs src/reconcile/worker/management.rs | wc -l)" -eq 1
done
rg -q '^pub const DATABASE_SCHEMA_VERSION: i64 = 7;' src/catalog/types.rs
rg -q '^pub const OPERATION_SCHEMA_VERSION: i64 = 2;' src/catalog/control.rs
test "$(rg -c '^CREATE TABLE ' src/catalog/schema_v7.sql)" -eq 2
if rg 'foreign_keys.*(false|OFF)|writable_schema|ALTER TABLE' src/catalog/store.rs; then exit 1; fi
rustfmt --edition 2021 --config skip_children=true --check src/catalog/control.rs src/catalog/store.rs src/catalog/policy_commands.rs src/catalog/policy_command_tests.rs src/catalog/policy_command_migration_tests.rs src/model_catalog.rs src/reconcile/worker.rs src/reconcile/worker/management.rs src/reconcile/worker/management_tests.rs
bash scripts/tests/performance-gates-regression.sh
expected="$(printf '%s\n' \
  catalog::store::tests::policy_commands::c2_pin_scope_noop_replay_and_local_receipt_are_exact \
  catalog::store::tests::policy_commands::c2_policy_commands_reject_wrong_scope_stale_versions_and_unmanaged_pin \
  catalog::store::tests::policy_commands::c2_clear_keeps_limit_pause_and_replay_does_not_advance_next_day \
  catalog::store::tests::policy_commands::c2_edit_uninitialized_policy_does_not_create_fake_runtime_and_invalidates_version \
  catalog::store::tests::policy_commands::c2_local_faults_roll_back_configuration_history_and_receipts \
  catalog::store::tests::policy_commands::c2_model_results_use_stable_request_identity_and_preserve_other_sources \
  catalog::store::tests::policy_commands::c2_schema_views_and_private_discovery_input \
  catalog::store::tests::policy_commands::c2_revision_boundaries_pause_and_retirement_fail_without_partial_changes \
  catalog::store::tests::policy_commands::c2_pb1_local_actions_match_independent_versions_and_replay \
  catalog::store::tests::policy_commands::c2_pb3_discovered_models_match_reference_and_replays_are_readonly \
  catalog::store::tests::policy_commands::migration::c2_pb2_v6_migration_preserves_all_records_backups_and_reopen \
  catalog::store::tests::policy_commands::migration::c2_v7_migration_faults_preserve_v6_and_foreign_keys \
  catalog::store::tests::policy_commands::migration::performance_gate_c2_v6_migration_10k_operations \
  reconcile::worker::tests::management_tests::c2_worker_pin_waits_for_policy_and_keeps_original_release_rule \
  reconcile::worker::tests::management_tests::c2_worker_clear_reenables_only_after_observed_publication_and_readback \
  reconcile::worker::tests::management_tests::c2_worker_models_use_one_get_and_original_multigroup_execution \
  reconcile::worker::tests::management_tests::c2_worker_bad_model_responses_leave_catalog_unchanged_and_redact_errors \
  reconcile::worker::tests::management_tests::c2_worker_models_no_discovery_config_redirect_or_dry_run_requests \
  reconcile::worker::tests::management_tests::c2_worker_model_response_cannot_overwrite_pause_retire_or_new_models \
  reconcile::worker::tests::management_tests::c2_worker_model_cancellation_and_singleflight_leave_no_half_operation \
  reconcile::worker::tests::management_tests::c2_worker_model_timeout_is_bounded_and_does_not_publish \
  reconcile::worker::tests::management_tests::c2_worker_local_commit_failure_keeps_cache_and_unknown_commit_closes_gate \
  reconcile::worker::tests::management_tests::c2_pb4_worker_policy_edits_invalidate_only_affected_source_versions | LC_ALL=C sort)"
expected_ignored="$(printf '%s\n' "$expected" | rg '::performance_gate_')"
manifest_ok() { [[ -n "$1" && "$1" == "$expected" && "$2" == "$expected_ignored" && "$1" != "$2" ]]; }
if manifest_ok '' '' || manifest_ok "$expected" "$expected" || manifest_ok "${expected#*$'\n'}" "$expected_ignored"; then exit 1; fi
listing="$(cargo test --locked --offline -j 2 -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^(catalog::store::tests::policy_commands::|reconcile::worker::tests::management_tests::)' | LC_ALL=C sort || true)"
listing="$(cargo test --locked --offline -j 2 -- --ignored --list)"
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^(catalog::store::tests::policy_commands::|reconcile::worker::tests::management_tests::)' | LC_ALL=C sort || true)"
if ! manifest_ok "$actual" "$ignored"; then printf 'C2 manifest mismatch\n' >&2; exit 1; fi
schema=docs/design/source-groups-token-management/schemas/control-operation.schema.json
output="$(cargo test --locked --offline -j 2 c2_schema_views_and_private_discovery_input -- --nocapture)"
views="$(printf '%s\n' "$output" | sed -n 's/^C2_SCHEMA_JSON=//p')"
test "$(printf '%s\n' "$views" | jq 'length')" -eq 4
while IFS= read -r view; do check-jsonschema --schemafile "$schema" --force-filetype json <(printf '%s\n' "$view"); done < <(printf '%s\n' "$views" | jq -c '.[]')
check-jsonschema --schemafile "$schema" docs/design/source-groups-token-management/schemas/control-operation.example.json
output="$(cargo test --locked --offline -j 2 control_operation_schema_output -- --nocapture)"
enums="$(printf '%s\n' "$output" | sed -n 's/^CONTROL_ENUM_JSON=//p')"
test "$(printf '%s\n' "$enums" | jq -c '.kind|sort')" == "$(jq -c '.properties.kind.enum|sort' "$schema")"
db_kinds="$(sed -n 's/^    kind TEXT NOT NULL CHECK(kind IN (\(.*\))),$/\1/p' src/catalog/schema_v7.sql | jq -Rc 'split(", ")|map(ltrimstr("\u0027")|rtrimstr("\u0027"))|sort')"
test "$db_kinds" == "$(printf '%s\n' "$enums" | jq -c '.kind|sort')"
bad="$(printf '%s\n' "$views" | jq -c '.[0] | .schema_version=1')"
if check-jsonschema --schemafile "$schema" --force-filetype json <(printf '%s\n' "$bad") >/dev/null 2>&1; then exit 1; fi
bad="$(printf '%s\n' "$views" | jq -c '.[0] | .total_targets=1')"
if check-jsonschema --schemafile "$schema" --force-filetype json <(printf '%s\n' "$bad") >/dev/null 2>&1; then exit 1; fi
printf 'C2 scope, frozen algorithms/DDL, schema/enum/Step and exact 23-test manifest passed; full regression, mutations and performance are separate.\n'
