#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=9595837
git diff --check "$base"
git diff --exit-code "$base" -- Cargo.toml Cargo.lock src \
  ':(exclude)src/catalog/mod.rs' ':(exclude)src/catalog/types.rs' ':(exclude)src/catalog/store.rs' \
  ':(exclude)src/catalog/ledger.rs' ':(exclude)src/catalog/ledger/' ':(exclude)src/catalog/schema_v5.sql' \
  ':(exclude)src/catalog/ledger_tests.rs' ':(exclude)src/catalog/ledger_properties.rs' ':(exclude)src/catalog/ledger_migration_tests.rs' \
  ':(exclude)src/catalog/tests.rs' ':(exclude)src/catalog/control_tests.rs' ':(exclude)src/catalog/execution_tests.rs' \
  ':(exclude)src/catalog/policy_tests.rs' ':(exclude)src/catalog/policy_migration_tests.rs' \
  ':(exclude)src/ledger/mod.rs' ':(exclude)src/ledger/decode.rs' ':(exclude)src/ledger/storage_codec.rs'
diff -u <(git show "$base:src/catalog/types.rs") <(sed 's/^pub const DATABASE_SCHEMA_VERSION: i64 = 5;$/pub const DATABASE_SCHEMA_VERSION: i64 = 4;/' src/catalog/types.rs)
diff -u <(git show "$base:src/catalog/mod.rs") <(sed '/^pub(crate) mod ledger;$/d' src/catalog/mod.rs)
for path in src/catalog/schema_v{1,2,3,4}.sql; do git diff --exit-code "$base" -- "$path"; done
while IFS= read -r path; do
  case "$path" in
    src/catalog/ledger.rs|src/catalog/ledger/*|src/catalog/ledger_tests.rs|src/catalog/ledger_properties.rs|src/catalog/ledger_migration_tests.rs|src/catalog/schema_v5.sql|src/ledger/storage_codec.rs|scripts/contract_audit/sgm4_b1.sh|docs/audits/sgm-4-facts-analytics/*|proptest-regressions/catalog/*) ;;
    *) printf 'Unexpected untracked file: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <(git ls-files --others --exclude-standard)
test "$(rg -c '// Step B1.1:' src/catalog/store.rs)" -eq 1
for step in 2 3 4 5; do test "$(rg -c "// Step B1.$step:" src/catalog/ledger.rs)" -eq 1; done
test "$(rg -c '// Step B1.6:' src/ledger/storage_codec.rs)" -eq 1
test "$(rg -c '^CREATE TABLE ' src/catalog/schema_v5.sql)" -eq 5
if rg 'reqwest|tokio|SystemTime|VolcanoEvidence|FinalOutcome|RoutingPermit|BackendReadiness' src/catalog/ledger.rs src/catalog/ledger/{types,rows,verify}.rs src/ledger/storage_codec.rs; then exit 1; fi
if rg 'enum (FactKind|SourceKind|AttributionIssue|StreamObservation)|const JSON_SAFE|struct (SourceId|GroupId|ChannelId)' src/catalog/ledger.rs src/catalog/ledger/{types,rows,verify}.rs src/ledger/storage_codec.rs; then exit 1; fi
rustfmt --edition 2021 --check src/catalog/ledger.rs src/catalog/ledger_tests.rs src/catalog/ledger_migration_tests.rs src/ledger/mod.rs src/catalog/store.rs
bash -n scripts/tests/performance-gates.sh scripts/tests/performance-gates-regression.sh
bash scripts/tests/performance-gates-regression.sh
expected="$(printf '%s\n' \
  catalog::store::tests::ledger::smoke_ingest_reopen_and_replay \
  catalog::store::tests::ledger::checkpoints_reject_stale_foreign_and_backwards_events \
  catalog::store::tests::ledger::same_identity_conflicts_in_every_field_without_partial_writes \
  catalog::store::tests::ledger::late_overflow_and_sql_failure_roll_back_facts_cells_and_checkpoint \
  catalog::store::tests::ledger::deferred_commit_failure_poisoning_requires_reopen \
  catalog::store::tests::ledger::retired_mapping_group_mismatch_and_newly_resolvable_unknown_survive_open \
  catalog::store::tests::ledger::null_cell_unique_constraint_and_metadata_redaction_are_enforced \
  catalog::store::tests::ledger::corrupt_rows_missing_state_and_projection_mismatch_are_not_repaired \
  catalog::store::tests::ledger::known_attribution_corruption_is_rejected_even_when_projection_matches \
  catalog::store::tests::ledger::unused_invalid_history_is_storage_corruption \
  catalog::store::tests::ledger::checkpoint_number_and_missing_table_corruption_are_rejected \
  catalog::store::tests::ledger::stored_codec_roundtrip_boundaries_and_late_row_corruption \
  catalog::store::tests::ledger::maximum_ingestion_capacity_without_wall_clock_budget \
  catalog::store::tests::ledger::performance_gate_ledger_ingestion \
  catalog::store::tests::ledger::performance_gate_ledger_reopen_100k \
  catalog::store::tests::ledger::properties::pb1_cross_batch_dedup_replay_and_reopen \
  catalog::store::tests::ledger::properties::pb2_late_failure_is_atomic \
  catalog::store::tests::ledger::properties::pb3_historical_attribution_preserves_shared_and_unknown \
  catalog::store::tests::ledger::properties::pb4_joint_cells_match_independent_raw_model \
  catalog::store::tests::ledger::properties::pb5_checkpoint_state_machine_replay_and_restart \
  catalog::store::tests::policy::migration::ledger::pb6_v1_to_v4_complex_migration_is_lossless \
  catalog::store::tests::policy::migration::ledger::v5_migration_failure_keeps_v4_and_original_backup \
  catalog::store::tests::policy::migration::ledger::v4_corruption_is_rejected_before_v5_backup \
  catalog::store::tests::policy::migration::ledger::performance_gate_ledger_v4_migration \
  | LC_ALL=C sort)"
listing="$(cargo test --locked --offline -j 2 ledger:: -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^catalog::store::tests::(policy::migration::)?ledger::' | LC_ALL=C sort || true)"
test "$actual" = "$expected"
listing="$(cargo test --locked --offline -j 2 ledger:: -- --ignored --list)"
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^catalog::store::tests::(policy::migration::)?ledger::' | LC_ALL=C sort || true)"
test "$ignored" = "$(printf '%s\n' "$expected" | rg '::performance_gate_')"
cargo test --locked --offline -j 2 ledger:: -- --nocapture
printf 'SGM-4 B1 mechanical checks passed; default full regression, performance and independent audit remain separate\n'
