#!/usr/bin/env bash
# 常规测试保留功能/容量断言；此入口完整、串行执行各批性能预算。
set -euo pipefail
if (( $# != 0 )); then
  printf 'Usage: bash scripts/tests/performance-gates.sh\n' >&2
  exit 2
fi
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."

expected="$(printf '%s\n' \
  catalog::store::tests::control::performance_gate_control_group_priorities \
  panel_http::publication::tests::capacity::performance_gate_panel_http_encoding \
  panel_http::publication::tests::capacity::performance_gate_panel_http_body \
  catalog::store::tests::performance_gate_import_and_snapshot \
  catalog::store::tests::execution::performance_gate_group_work_snapshot \
  source_policy::tests::performance_gate_glm_pure_capacity \
  catalog::store::tests::panel::performance_gate_panel_maximum_catalog \
  catalog::store::tests::glm_round::performance_gate_glm_maximum_catalog \
  catalog::store::tests::tokens::migration::performance_gate_token_identity_10k \
  catalog::store::tests::tokens::migration::performance_gate_token_v7_migration_10k_operations \
  catalog::store::tests::tokens::service::performance_gate_group_references_10k \
  newapi::groups::tests::performance_gate_group_options_10k \
  catalog::store::tests::policy_commands::migration::performance_gate_c2_v6_migration_10k_operations \
  catalog::store::tests::observed_publication::scale::performance_gate_observed_period_usage_100k \
  catalog::store::tests::observed_publication::scale::performance_gate_observed_maximum_catalog \
  catalog::store::tests::ledger::performance_gate_ledger_ingestion \
  catalog::store::tests::ledger::performance_gate_ledger_reopen_100k \
  catalog::store::tests::ledger::scans::performance_gate_collection_page \
  catalog::store::tests::ledger::scans::performance_gate_collection_restart_100k \
  catalog::store::tests::ledger::queries::scale::performance_gate_analytics_sql_100k \
  catalog::store::tests::ledger::attribution::performance_gate_attribution_batch_10k_history \
  catalog::store::tests::policy::migration::ledger::performance_gate_ledger_v4_migration \
  catalog::store::tests::policy::performance_gate_policy_v3_migration \
  catalog::store::tests::policy::migration::performance_gate_complex_history_migration \
  ledger::analytics::tests::performance_gate_analytics_maximum_output \
  ledger::observations::tests::performance_gate_observed_breaker_100k \
  ledger::tests::performance_gate_ledger_maximum_batch \
  newapi::tokens::tests::performance_gate_token_maximum_page \
  source_policy::volcano::tests::performance_gate_volcano_maximum_selection | LC_ALL=C sort)"
listing="$(cargo test --release --locked --bin quota-throttle performance_gate_ -- --ignored --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | LC_ALL=C sort)"
if [[ "$actual" != "$expected" ]]; then
  printf 'Performance gate set mismatch; expected exactly:\n%s\n' "$expected" >&2
  exit 1
fi
cargo test --release --locked --bin quota-throttle performance_gate_ -- --ignored --test-threads=1 --nocapture

# 旧大组/导入原预算也约束debug，不能只移到release后视为同一门禁。
for debug_gate in \
  catalog::store::tests::control::performance_gate_control_group_priorities \
  catalog::store::tests::performance_gate_import_and_snapshot \
  catalog::store::tests::execution::performance_gate_group_work_snapshot \
  source_policy::tests::performance_gate_glm_pure_capacity; do
  listing="$(cargo test --locked --bin quota-throttle "$debug_gate" -- --ignored --exact --list)"
  actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p')"
  if [[ "$actual" != "$debug_gate" ]]; then
    printf 'Debug performance gate mismatch; expected exactly: %s\n' "$debug_gate" >&2
    exit 1
  fi
  cargo test --locked --bin quota-throttle "$debug_gate" -- --ignored --exact --test-threads=1 --nocapture
done
