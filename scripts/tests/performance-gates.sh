#!/usr/bin/env bash
# 常规测试保留功能/容量断言；此入口完整、串行执行各批性能预算。
set -euo pipefail
if (( $# != 0 )); then
  printf 'Usage: bash scripts/tests/performance-gates.sh\n' >&2
  exit 2
fi
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."

expected="$(printf '%s\n' \
  catalog::store::tests::glm_round::performance_gate_glm_maximum_catalog \
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
  ledger::tests::performance_gate_ledger_maximum_batch \
  source_policy::volcano::tests::performance_gate_volcano_maximum_selection | LC_ALL=C sort)"
listing="$(cargo test --release --locked --bin quota-throttle performance_gate_ -- --ignored --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | LC_ALL=C sort)"
if [[ "$actual" != "$expected" ]]; then
  printf 'Performance gate set mismatch; expected exactly:\n%s\n' "$expected" >&2
  exit 1
fi
cargo test --release --locked --bin quota-throttle performance_gate_ -- --ignored --test-threads=1 --nocapture
