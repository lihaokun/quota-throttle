#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=bc8f33e
git diff --check "$base"
while IFS= read -r path; do
  case "$path" in
    src/catalog/control_tests.rs|docs/fixes/catalog-fix-group-operation-performance-gate.md) ;;
    src/catalog/groups.rs|src/catalog/mod.rs|src/catalog/token_service_tests.rs|src/catalog/token_tests.rs|src/catalog/tokens.rs|src/catalog/tokens/execution.rs|src/newapi.rs|src/newapi/groups.rs|src/newapi/groups/*|src/newapi/transport.rs|src/reconcile/worker.rs|src/reconcile/worker/tests.rs|src/reconcile/worker/token_service.rs|src/reconcile/worker/token_service/*|src/reconcile/worker/token_service_tests.rs|src/reconcile/worker/token_service_tests/*|docs/design/source-groups-token-management/*|docs/audits/sgm-5-token-management/c-*|docs/fixes/token-service-fix-admission-composition.md|scripts/contract_audit/sgm5_c.sh|scripts/probes/newapi_groups_probe.py|scripts/probes/newapi_token_service_probe.py|scripts/tests/performance-gates.sh|scripts/tests/performance-gates-regression.sh|proptest-regressions/newapi/groups/*|proptest-regressions/reconcile/worker/token_service_tests/*|proptest-regressions/catalog/token_service_tests.txt) ;;
    *) printf 'Unexpected SGM5 C path: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <({ git diff --name-only "$base"; git ls-files --others --exclude-standard; } | LC_ALL=C sort -u)
git diff --exit-code "$base" -- Cargo.toml Cargo.lock src/main.rs src/boot.rs src/config.rs src/status.rs src/orchestrator.rs src/quota.rs src/source_policy src/ledger src/catalog/schema_v1.sql src/catalog/schema_v2.sql src/catalog/schema_v3.sql src/catalog/schema_v4.sql src/catalog/schema_v5.sql src/catalog/schema_v6.sql src/catalog/schema_v7.sql src/catalog/schema_v8.sql src/catalog/store.rs src/catalog/control.rs src/catalog/confirmation.rs src/catalog/execution.rs src/catalog/policy.rs src/catalog/policy_commands.rs src/catalog/glm_round.rs src/catalog/glm_publication.rs src/catalog/volcano_publication.rs src/catalog/ledger.rs src/catalog/ledger src/newapi/tokens.rs src/newapi/tokens src/newapi/transport src/newapi/managed_channels.rs src/newapi/managed_channels src/newapi/preflight.rs src/newapi/preflight src/newapi/logs.rs src/newapi/logs src/reconcile/planner.rs src/reconcile/planner src/reconcile/worker/policy.rs src/reconcile/worker/policy_tests.rs src/reconcile/worker/management.rs src/reconcile/worker/management_tests.rs src/reconcile/worker/analytics.rs src/reconcile/worker/analytics src/reconcile/worker/collection.rs src/reconcile/worker/collection src/reconcile/worker/volcano_tests.rs src/reconcile/worker/scenarios.rs src/reconcile/worker/acquisition_tests.rs
diff -u <(git show "$base:src/catalog/mod.rs") <(sed '/^pub(crate) mod groups;$/d' src/catalog/mod.rs)
# 独立修正遗漏的旧性能门禁：所有旧夹具/值断言和计时位置必须字节等价。
control_test=src/catalog/control_tests.rs
test "$(sed -n '/^fn full_group_operation_is_bounded_and_preserves_each_priority()/,/^}$/p' "$control_test")" == $'fn full_group_operation_is_bounded_and_preserves_each_priority() {\n    full_group_operation_case();\n}'
test "$(sed -n '/^fn performance_gate_control_group_priorities()/,/^}$/p' "$control_test")" == $'fn performance_gate_control_group_priorities() {\n    let elapsed = full_group_operation_case();\n    println!("CONTROL_GROUP_PRIORITY_MAXIMUM_MS={}", elapsed.as_millis());\n    assert!(elapsed < Duration::from_secs(5));\n}'
diff -u <(git show "$base:$control_test") <(awk '
  /^fn full_group_operation_is_bounded_and_preserves_each_priority\(\)/ {print;skip=1;next}
  skip && /^fn full_group_operation_case\(\) -> Duration {/ {skip=0;in_case=1;next}
  skip {next}
  in_case && /^    let elapsed = started.elapsed\(\);$/ {print "    assert!(started.elapsed() < Duration::from_secs(5));";next}
  in_case && /^    elapsed$/ {next}
  in_case && /^}$/ {in_case=0}
  {print}
' "$control_test")
diff -u <(git show "$base:src/catalog/token_tests.rs") <(sed '/^#\[path = "token_service_tests.rs"\]$/,+1d' src/catalog/token_tests.rs)
diff -u <(git show "$base:src/reconcile/worker/tests.rs") <(sed '/^#\[path = "token_service_tests.rs"\]$/,+1d' src/reconcile/worker/tests.rs)
strip_methods() {
  awk -v pattern="$1" '
    $0 ~ pattern {skip=1;next}
    skip && /^    }$/ {skip=0;next}
    !skip {print}
  ' "$2"
}
diff -u <(git show "$base:src/newapi/transport.rs") <(strip_methods '^    pub[(]super[)] fn shares_scope[(]' src/newapi/transport.rs)
diff -u <(git show "$base:src/newapi.rs") <(strip_methods '^    pub[(]crate[)] fn (managed_groups|shares_management_scope)' src/newapi.rs | sed '/^pub(crate) mod groups;$/d')
# enabled是旧Ticket的已有函数，只剥离Work新增访问器，不吞旧Ticket实现。
diff -u <(git show "$base:src/catalog/tokens.rs") <(awk '
    /^impl TokenWork {/ {in_work=1}
    in_work && /^}$/ {in_work=0}
    (in_work && /^    pub fn (group_id|loaded_at|kind|enabled)\(/) || /^    pub\(crate\) fn replayed_token_operation\(/ {skip=1;next}
    skip && /^    }$/ {skip=0;next}
    !skip {print}
  ' src/catalog/tokens.rs)
diff -u <(git show "$base:src/catalog/tokens/execution.rs" | sed '/^impl CatalogStore {/,$d') <(sed '/^impl CatalogStore {/,$d' src/catalog/tokens/execution.rs)
diff -u <(git show "$base:src/catalog/tokens/execution.rs" | sed -n '/^    pub(crate) fn prepare_token_execution(/,$p') <(sed -n '/^    pub(crate) fn prepare_token_execution(/,$p' src/catalog/tokens/execution.rs)
previous="$(git show "$base:src/reconcile/worker.rs" | sed -n '/^            let next = self$/,/^                \.next_attempt_at()?;$/p')"
test -n "$previous"
diff -u <(git show "$base:src/reconcile/worker.rs") <(awk -v previous="$previous" '
  /^pub\(crate\) mod token_service;$/ || /^    token_owners:/ || /^            token_owners: None,$/ || /^        self.token_round\(stop, &mut report\).await\?;$/ {next}
  /^        if view.target_type == crate::catalog::control::TargetType::Token {/ {retry=1;next}
  retry && /^        }$/ {retry=0;next}
  retry {next}
  /^            let next = {/ {timing=1;print previous;next}
  timing && /^            };$/ {timing=0;next}
  timing {next}
  {print}
' src/reconcile/worker.rs)
for step in T5C.1 T5C.2 T5C.3 T5C.4 T5C.5 T5C.6; do
  test "$(rg -F "// Step $step:" src/newapi/groups.rs src/catalog/groups.rs src/reconcile/worker/token_service.rs src/reconcile/worker/token_service | wc -l)" -eq 1
done
rg -q '^pub const GROUP_CHECK_SCHEMA_VERSION: i64 = 1;' src/reconcile/worker/token_service.rs
rg -q '^pub const GROUP_REFERENCES_SCHEMA_VERSION: i64 = 1;' src/catalog/groups.rs
rg -q '^pub const DATABASE_SCHEMA_VERSION: i64 = 8;' src/catalog/types.rs
rg -q '^pub const OPERATION_SCHEMA_VERSION: i64 = 3;' src/catalog/control.rs
rustfmt --edition 2021 --config skip_children=true --check src/newapi/groups.rs src/newapi/groups/tests.rs src/catalog/groups.rs src/catalog/token_service_tests.rs src/catalog/tokens.rs src/catalog/tokens/execution.rs src/reconcile/worker/token_service.rs src/reconcile/worker/token_service/execution.rs src/reconcile/worker/token_service/access.rs src/reconcile/worker/token_service_tests.rs src/reconcile/worker/token_service_tests/coverage.rs src/reconcile/worker/token_service_tests/edges.rs src/reconcile/worker/token_service_tests/native.rs
bash scripts/tests/performance-gates-regression.sh
n=newapi::groups::tests
c=catalog::store::tests::tokens::service
w=reconcile::worker::tests::token_service_tests
expected="$(printf '%s\n' \
 "$n::t5c_group_options_reject_missing_duplicate_bad_and_oversized_maps" \
 "$n::t5c_group_merge_is_idempotent_and_preserves_unrelated_options" \
 "$n::t5c_group_alias_ambiguity_and_denial_send_no_put" \
 "$n::t5c_group_partial_or_unknown_write_never_grants_or_repairs_on_retry" \
 "$n::t5c_group_final_guard_invalidates_before_any_write" \
 "$n::t5c_group_invalid_input_is_network_free_and_scope_requires_same_budget" \
 "$n::t5c_pb1_group_merge_matches_independent_add_only_reference" \
 "$n::t5c_pb2_group_conflicts_never_choose_alias_or_remove_deny" \
 "$n::performance_gate_group_options_10k" \
 "$c::t5c_group_ready_versions_noops_and_errors_are_atomic" \
 "$c::t5c_group_reference_schema_counts_pending_unmapped_legacy_and_history" \
 "$c::t5c_token_access_uses_current_revision_but_rejects_static_drift" \
 "$c::t5c_delay_cannot_refresh_observation_or_clear_unknown" \
 "$c::t5c_delay_rollback_and_safe_clock_boundary" \
 "$c::t5c_pb3_group_reference_and_ready_changes_match_independent_model" \
 "$c::performance_gate_group_references_10k" \
 "$w::t5c_positive_admission_uses_later_live_candidate" \
 "$w::t5c_worker_idempotency_conflict_precedes_missing_target_and_owner" \
 "$w::t5c_group_configuration_does_not_confuse_row_age_with_drift" \
 "$w::t5c_volcano_auto_ban_is_not_a_ready_configuration" \
 "$w::coverage::t5c_create_replay_uses_persistent_owner_when_issuer_missing" \
 "$w::coverage::t5c_create_ack_requires_lookup_and_explicit_secret_return" \
 "$w::coverage::t5c_disable_delete_ignore_balance_group_and_source_outage" \
 "$w::coverage::t5c_enable_validates_static_identity_before_put" \
 "$w::coverage::t5c_unknown_status_matching_get_retry_and_opposite_command_stay_blocked" \
 "$w::coverage::t5c_missing_issuer_preserves_replay_and_legacy_management_without_fallback" \
 "$w::coverage::t5c_missing_owner_and_read_failures_are_throttled" \
 "$w::coverage::t5c_group_scan_checks_tail_unknown_enabled_and_preserves_noop_permits" \
 "$w::coverage::t5c_reveal_checks_after_secret_response_and_disabled_does_not_enable" \
 "$w::coverage::t5c_prepare_then_failed_check_is_not_sent_and_not_left_applying" \
 "$w::coverage::t5c_drop_cancellation_and_outer_cancellation_preserve_delivery" \
 "$w::coverage::t5c_shutdown_settles_dispatched_write_and_leaves_next_queued" \
 "$w::coverage::t5c_pb4_current_owner_group_and_live_candidate_gate_writes" \
 "$w::coverage::t5c_pb5_creation_delivery_reopen_and_replay_never_duplicate" \
 "$w::coverage::t5c_pb6_legacy_management_preserves_group_and_unrelated_catalog" \
 "$w::edges::t5c_owners_are_private_validate_scope_and_allow_legacy_only" \
 "$w::edges::t5c_volcano_positive_gate_requires_auto_ban_off_but_glm_keeps_it" \
 "$w::edges::t5c_dry_run_and_backpressure_do_not_publish_or_send" \
 "$w::edges::t5c_final_gate_rejects_post_after_last_read_changes_publication" \
 "$w::edges::t5c_401_recovery_rechecks_worker_gate_before_create_replay" \
 "$w::edges::t5c_missing_ambiguous_and_expired_creation_readback_never_resends" \
 "$w::edges::t5c_current_group_revision_and_lost_query_do_not_rewrite_receipts" \
 "$w::edges::t5c_actual_past_creation_expiry_recovers_without_current_routing" \
 "$w::edges::t5c_token_management_preserves_nonempty_ledger_and_all_old_sql_rows" \
 "$w::native::t5c_native_token_service_combination" | LC_ALL=C sort)"
expected_ignored="$(printf '%s\n' "$n::performance_gate_group_options_10k" "$c::performance_gate_group_references_10k" "$w::native::t5c_native_token_service_combination" | LC_ALL=C sort)"
test "$(printf '%s\n' "$expected" | wc -l)" -eq 45
manifest_ok() { [[ -n "$1" && "$1" == "$expected" && "$2" == "$expected_ignored" && "$1" != "$2" ]]; }
if manifest_ok '' '' || manifest_ok "$expected" "$expected" || manifest_ok "${expected#*$'\n'}" "$expected_ignored"; then exit 1; fi
listing="$(cargo test --locked --offline -j 2 -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg "^($n|$c|$w)::" | LC_ALL=C sort || true)"
listing="$(cargo test --locked --offline -j 2 -- --ignored --list)"
printf '%s\n' "$listing" | rg -q '^catalog::store::tests::control::performance_gate_control_group_priorities: test$'
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg "^($n|$c|$w)::" | LC_ALL=C sort || true)"
manifest_ok "$actual" "$ignored" || { printf 'SGM5 C manifest mismatch\n' >&2; exit 1; }
test "$(printf '%s\n' "$actual" | rg -c '::t5c_pb[1-6]_')" -eq 6
output="$(cargo test --locked --offline -j 2 "$c::t5c_group_reference_schema_counts_pending_unmapped_legacy_and_history" -- --exact --nocapture)"
refs="$(printf '%s\n' "$output" | sed -n 's/^GROUP_REFERENCES_SCHEMA_JSON=//p')"
output="$(cargo test --locked --offline -j 2 "$w::coverage::t5c_group_scan_checks_tail_unknown_enabled_and_preserves_noop_permits" -- --exact --nocapture)"
report="$(printf '%s\n' "$output" | sed -n 's/^GROUP_CHECK_SCHEMA_JSON=//p')"
codes="$(printf '%s\n' "$output" | sed -n 's/^GROUP_CODE_ENUM_JSON=//p')"
schema=docs/design/source-groups-token-management/schemas
check-jsonschema --schemafile "$schema/group-references.schema.json" --force-filetype json <(printf '%s\n' "$refs")
check-jsonschema --schemafile "$schema/group-check.schema.json" --force-filetype json <(printf '%s\n' "$report")
test "$(printf '%s\n' "$codes" | jq -c 'sort')" == "$(jq -c '[.properties.error_code.enum[]|select(.!=null)]|sort' "$schema/group-check.schema.json")"
for expression in '.key="plaintext"' '.known_local_only=false' '.deletion_authorized=true' '.live_tokens=10001' '.pending_creations=-1' '.group_id=null'; do
  bad="$(printf '%s\n' "$refs" | jq -c "$expression")"
  if check-jsonschema --schemafile "$schema/group-references.schema.json" --force-filetype json <(printf '%s\n' "$bad") >/dev/null 2>&1; then exit 1; fi
done
for expression in '.key="plaintext"' '.schema_version=2' '.ready=false' '.error_code="unknown"' '.error_code="not_ready"'; do
  bad="$(printf '%s\n' "$report" | jq -c "$expression")"
  if check-jsonschema --schemafile "$schema/group-check.schema.json" --force-filetype json <(printf '%s\n' "$bad") >/dev/null 2>&1; then exit 1; fi
done
printf 'SGM5 C frozen scope, two schemas/enums, six Steps/PBT and exact 45-entry manifest passed; full regression/mutation/native/performance remain separate.\n'
