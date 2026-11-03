#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=f0b8295
git diff --check "$base"
git diff --exit-code "$base" -- src Cargo.toml Cargo.lock \
  ':(exclude)src/main.rs' ':(exclude)src/ledger/' ':(exclude)src/newapi/transport/tests.rs'
# 独立修正仅允许该测试函数；不放开整份认证测试或生产transport。
auth_test='/^async fn authentication_waiters_are_bounded_and_cancelled_login_opens_circuit() {$/,/^}$/d'
test "$(rg -c '^async fn authentication_waiters_are_bounded_and_cancelled_login_opens_circuit\(\) \{$' src/newapi/transport/tests.rs)" -eq 1
diff -u <(git show "$base:src/newapi/transport/tests.rs" | sed "$auth_test") \
  <(sed "$auth_test" src/newapi/transport/tests.rs)
test "$(rg -c '^mod ledger;$' src/main.rs)" -eq 1
diff -u <(git show "$base:src/main.rs") <(sed '/^mod ledger;$/d' src/main.rs)
while IFS= read -r path; do
  case "$path" in
    src/ledger/*|scripts/contract_audit/sgm4_a.sh|docs/audits/sgm-4-facts-analytics/*|proptest-regressions/ledger/*|docs/fixes/newapi-fix-auth-waiter-test-sync.md) ;;
    *) printf 'Unexpected untracked file: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <(git ls-files --others --exclude-standard)
test "$(rg -c '// Step A1:' src/ledger/decode.rs)" -eq 1
for step in 2 3 4 5; do test "$(rg -c "// Step A$step:" src/ledger/mod.rs)" -eq 1; done
if rg 'reqwest|rusqlite|std::fs|SystemTime|tokio|\.await|VolcanoEvidence|FinalOutcome|RoutingPermit|BackendReadiness' src/ledger/{mod,decode,types}.rs; then
  printf 'Ledger A must remain pure and cannot produce routing evidence\n' >&2; exit 1
fi
if rg 'enum (SourceKind|OutcomeKind|Regime)|struct (SourceId|GroupId|ChannelId)|const JSON_SAFE' src/ledger/{mod,decode,types}.rs; then
  printf 'Shared types/constants must be imported\n' >&2; exit 1
fi
rustfmt --edition 2021 --check src/ledger/mod.rs
bash -n scripts/tests/performance-gates.sh scripts/tests/performance-gates-regression.sh
bash scripts/tests/performance-gates-regression.sh
# 函数名与忽略状态都要核验，防止空筛选或把普通测试全部ignore后假绿。
expected="$(printf 'ledger::tests::%s\n' \
  core_fields_are_required_strict_and_bounded \
  duplicate_conflicts_check_every_canonical_field \
  error_and_unrelated_rows_cannot_become_consumption \
  handmade_facts_and_attributions_cannot_bypass_checks \
  history_validation_precedes_filtering_and_preserves_unknown_groups \
  label_degradation_keeps_usage_and_other_valid_dimensions \
  maximum_batch_and_history_are_bounded_without_truncation \
  metadata_boundaries_are_degraded_not_silently_counted \
  overflow_in_late_cell_returns_no_partial_delta \
  performance_gate_ledger_maximum_batch \
  private_metadata_has_no_debug_display_or_wire_traits \
  request_ids_are_optional_bounded_and_not_synthesized \
  stream_and_retry_metadata_never_reclassify_usage \
  properties::p1_parse_preserves_usage_and_redacts_invalid_input \
  properties::p2_deduplication_is_order_independent_and_conflicts_are_rejected \
  properties::p3_historical_attribution_is_total_and_checks_unused_mappings \
  properties::p4_joint_projection_and_all_marginals_conserve_usage \
  properties::p5_disjoint_batches_merge_without_losing_dimensions \
  properties::p6_exact_arithmetic_and_utc_minutes_preserve_boundaries | LC_ALL=C sort)"
listing="$(cargo test --locked --offline -j 2 ledger:: -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | LC_ALL=C sort)"
test "$actual" = "$expected"
listing="$(cargo test --locked --offline -j 2 ledger:: -- --ignored --list)"
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p')"
test "$ignored" = 'ledger::tests::performance_gate_ledger_maximum_batch'
cargo test --locked --offline -j 2 ledger:: -- --nocapture
printf 'SGM-4 A mechanical checks passed; full regression/performance/independent audit remain separate\n'
