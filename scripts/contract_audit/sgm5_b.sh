#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=ec17fc9
git diff --check "$base"
while IFS= read -r path; do
  case "$path" in
    src/catalog/tokens.rs|src/catalog/tokens/*|src/catalog/schema_v8.sql|src/catalog/token_tests.rs|src/catalog/token_migration_tests.rs|src/catalog/store.rs|src/catalog/control.rs|src/catalog/confirmation.rs|src/catalog/execution.rs|src/catalog/types.rs|src/catalog/mod.rs|src/catalog/tests.rs|src/catalog/policy_command_tests.rs|src/catalog/policy_command_migration_tests.rs|src/catalog/observed_publication_properties.rs|src/newapi/tokens.rs|docs/design/source-groups-token-management/*|docs/audits/sgm-5-token-management/*|docs/fixes/catalog-token-fix-lifecycle-evidence.md|scripts/contract_audit/sgm5_b.sh|scripts/tests/performance-gates.sh|scripts/tests/performance-gates-regression.sh|proptest-regressions/catalog/token*|proptest-regressions/catalog/tokens/*) ;;
    *) printf 'Unexpected SGM5 B path: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <({ git diff --name-only "$base"; git ls-files --others --exclude-standard; } | LC_ALL=C sort -u)
git diff --exit-code "$base" -- Cargo.toml Cargo.lock src/source_policy src/ledger src/reconcile src/main.rs src/boot.rs src/config.rs src/status.rs src/orchestrator.rs src/newapi.rs src/newapi/transport.rs src/newapi/transport src/newapi/managed_channels.rs src/newapi/managed_channels src/newapi/preflight.rs src/newapi/preflight src/newapi/logs.rs src/newapi/logs src/newapi/tokens src/catalog/schema_v1.sql src/catalog/schema_v2.sql src/catalog/schema_v3.sql src/catalog/schema_v4.sql src/catalog/schema_v5.sql src/catalog/schema_v6.sql src/catalog/schema_v7.sql src/catalog/policy.rs src/catalog/policy_commands.rs src/catalog/glm_round.rs src/catalog/glm_publication.rs src/catalog/volcano_publication.rs src/catalog/ledger.rs src/catalog/ledger
diff -u <(git show "$base:src/newapi/tokens.rs") <(sed -e 's/use serde::{Deserialize, Serialize};/use serde::Serialize;/' -e 's/Eq, Serialize, Deserialize)/Eq, Serialize)/' src/newapi/tokens.rs)
diff -u <(git show "$base:src/catalog/execution.rs") <(sed 's/^    pub(super) fn columns/    fn columns/' src/catalog/execution.rs)
diff -u <(git show "$base:src/catalog/types.rs") <(sed -e 's/DATABASE_SCHEMA_VERSION: i64 = 8/DATABASE_SCHEMA_VERSION: i64 = 7/' -e '/^id_type!(ManagedTokenId);$/d' src/catalog/types.rs)
diff -u <(git show "$base:src/catalog/mod.rs") <(sed '/^pub(crate) mod tokens;$/d' src/catalog/mod.rs)
diff -u <(git show "$base:src/catalog/control.rs" | sed -n '/^pub enum ControlCommand {/,/^pub(super) fn replayed_operation(/p') <(sed -n '/^pub enum ControlCommand {/,/^pub(super) fn replayed_operation(/p' src/catalog/control.rs)
diff -u <(git show "$base:src/catalog/control.rs" | sed -n '/^    pub fn submit_operation(/,/^pub(super) fn bump(/p') <(sed -n '/^    pub fn submit_operation(/,/^pub(super) fn bump(/p' src/catalog/control.rs)
diff -u <(git show "$base:src/catalog/store.rs" | sed -n '/^    pub fn create_source(/,/^fn verify_schema(/p') <(sed -n '/^    pub fn create_source(/,/^fn verify_schema(/p' src/catalog/store.rs)
diff -u <(git show "$base:src/catalog/confirmation.rs") <(awk '
  /^        if before.target_type == super::control::TargetType::Token {/ {skip=1;next}
  skip && /^        }$/ {skip=0;next}
  !skip {print}
' src/catalog/confirmation.rs)
for step in T5B.1 T5B.2 T5B.3 T5B.4 T5B.5 T5B.6; do test "$(rg -F "// Step $step:" src/catalog/tokens.rs src/catalog/tokens src/catalog/store.rs | wc -l)" -eq 1; done
rg -q '^pub const TOKEN_IDENTITY_SCHEMA_VERSION: i64 = 1;' src/catalog/tokens.rs
rg -q '^pub const OPERATION_SCHEMA_VERSION: i64 = 3;' src/catalog/control.rs
rg -q '^pub const DATABASE_SCHEMA_VERSION: i64 = 8;' src/catalog/types.rs
rustfmt --edition 2021 --config skip_children=true --check src/catalog/tokens.rs src/catalog/tokens/execution.rs src/catalog/tokens/verify.rs src/catalog/token_tests.rs src/catalog/token_migration_tests.rs
bash scripts/tests/performance-gates-regression.sh
prefix=catalog::store::tests::tokens
expected="$(printf '%s\n' \
  "$prefix::t5b_schema_owner_identity_and_private_material" \
  "$prefix::t5b_replay_precedes_expiry_revision_and_shared_namespace_conflict" \
  "$prefix::t5b_invalid_create_and_owner_scope_changes_are_atomic" \
  "$prefix::t5b_every_payload_field_participates_in_idempotency" \
  "$prefix::t5b_creation_missing_ambiguity_and_ack_never_retry_or_confirm" \
  "$prefix::t5b_not_sent_retry_is_safe_but_late_not_sent_cannot_erase_binding" \
  "$prefix::t5b_confirmed_prepared_create_and_delete_survive_reopen" \
  "$prefix::t5b_unsettled_status_cannot_be_washed_by_get_retry_or_new_command" \
  "$prefix::t5b_old_failed_retry_cannot_block_newer_success" \
  "$prefix::t5b_send_guard_and_read_fences_cover_time_and_evidence_changes" \
  "$prefix::t5b_read_diagnostics_throttle_candidates_and_consume_work_once" \
  "$prefix::t5b_legacy_registration_preserves_empty_auto_unknown_group_and_history" \
  "$prefix::t5b_import_cannot_steal_pending_creation_or_reassign_native_id" \
  "$prefix::t5b_malformed_or_wrong_scope_read_never_partially_binds" \
  "$prefix::t5b_transaction_failures_and_poison_do_not_publish_half_operations" \
  "$prefix::t5b_settle_confirm_retry_and_recovery_failures_are_atomic" \
  "$prefix::t5b_pb1_replays_and_creation_marker_match_independent_reference" \
  "$prefix::t5b_pb2_creation_delivery_reopen_and_expired_lookup_model" \
  "$prefix::t5b_pb3_status_sequences_fence_old_reads_and_preserve_receipts" \
  "$prefix::t5b_pb4_import_delete_keeps_identity_and_unrelated_history" \
  "$prefix::migration::t5b_pb5_v7_migration_keeps_complex_history_and_backup" \
  "$prefix::migration::t5b_migration_failures_keep_v7_and_reject_foreign_constraints" \
  "$prefix::migration::t5b_reopen_rejects_corrupt_identity_receipt_and_future_schema" \
  "$prefix::migration::t5b_reopen_requires_reverse_lifecycle_evidence" \
  "$prefix::migration::t5b_capacity_pagination_and_overflow_do_not_truncate_or_wrap" \
  "$prefix::migration::performance_gate_token_identity_10k" \
  "$prefix::migration::performance_gate_token_v7_migration_10k_operations" | LC_ALL=C sort)"
expected_ignored="$(printf '%s\n' "$prefix::migration::performance_gate_token_identity_10k" "$prefix::migration::performance_gate_token_v7_migration_10k_operations" | LC_ALL=C sort)"
manifest_ok() { [[ -n "$1" && "$1" == "$expected" && "$2" == "$expected_ignored" && "$1" != "$2" ]]; }
if manifest_ok '' '' || manifest_ok "$expected" "$expected" || manifest_ok "${expected#*$'\n'}" "$expected_ignored"; then exit 1; fi
listing="$(cargo test --locked --offline -j 2 -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^catalog::store::tests::tokens::' | LC_ALL=C sort || true)"
listing="$(cargo test --locked --offline -j 2 -- --ignored --list)"
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^catalog::store::tests::tokens::' | LC_ALL=C sort || true)"
manifest_ok "$actual" "$ignored" || { printf 'Token B manifest mismatch\n' >&2; exit 1; }
output="$(cargo test --locked --offline -j 2 t5b_schema_owner_identity_and_private_material -- --nocapture)"
view="$(printf '%s\n' "$output" | sed -n 's/^B_TOKEN_SCHEMA_JSON=//p')"
operation="$(printf '%s\n' "$output" | sed -n 's/^B_OPERATION_SCHEMA_JSON=//p')"
enums="$(printf '%s\n' "$output" | sed -n 's/^B_TOKEN_ENUM_JSON=//p')"
schema=docs/design/source-groups-token-management/schemas/token-identity.schema.json
operation_schema=docs/design/source-groups-token-management/schemas/control-operation.schema.json
check-jsonschema --schemafile "$schema" --force-filetype json <(printf '%s\n' "$view")
check-jsonschema --schemafile "$operation_schema" --force-filetype json <(printf '%s\n' "$operation")
for field in origin owner_scope create_state; do
  test "$(printf '%s\n' "$enums" | jq -c --arg f "$field" '.[$f]|sort')" == "$(jq -c --arg f "$field" '.properties[$f].enum|sort' "$schema")"
done
for field in kind target_type; do
  test "$(printf '%s\n' "$enums" | jq -c --arg f "$field" '.[$f]|sort')" == "$(jq -c --arg f "$field" '.properties[$f].enum|sort' "$operation_schema")"
done
for expression in '.key="plaintext"' '.schema_version=2' '.owner_scope="admin"' '.native_token_id=null' '.group_id=null'; do
  bad="$(printf '%s\n' "$view" | jq -c "$expression")"
  if check-jsonschema --schemafile "$schema" --force-filetype json <(printf '%s\n' "$bad") >/dev/null 2>&1; then exit 1; fi
done
for expression in '.schema_version=2' '.total_targets=0' '.phase="partial"' '.target_type="membership"'; do
  bad="$(printf '%s\n' "$operation" | jq -c "$expression")"
  if check-jsonschema --schemafile "$operation_schema" --force-filetype json <(printf '%s\n' "$bad") >/dev/null 2>&1; then exit 1; fi
done
printf 'SGM5 B scope, frozen old algorithms/DDL/HTTP, schema/enum/Step and exact 27-test manifest passed; regression, mutation and performance are separate.\n'
