#!/usr/bin/env bash
# 仅 mock cargo 的列表和退出码，不编译、不创建夹具文件、不访问网络。
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."

cargo() {
  local tests=(
    catalog::store::tests::glm_round::performance_gate_glm_maximum_catalog
    catalog::store::tests::observed_publication::scale::performance_gate_observed_period_usage_100k
    catalog::store::tests::observed_publication::scale::performance_gate_observed_maximum_catalog
    catalog::store::tests::ledger::performance_gate_ledger_ingestion
    catalog::store::tests::ledger::performance_gate_ledger_reopen_100k
    catalog::store::tests::ledger::scans::performance_gate_collection_page
    catalog::store::tests::ledger::scans::performance_gate_collection_restart_100k
    catalog::store::tests::ledger::queries::scale::performance_gate_analytics_sql_100k
    catalog::store::tests::ledger::attribution::performance_gate_attribution_batch_10k_history
    catalog::store::tests::policy::migration::ledger::performance_gate_ledger_v4_migration
    catalog::store::tests::policy::performance_gate_policy_v3_migration
    catalog::store::tests::policy::migration::performance_gate_complex_history_migration
    ledger::analytics::tests::performance_gate_analytics_maximum_output
    ledger::observations::tests::performance_gate_observed_breaker_100k
    ledger::tests::performance_gate_ledger_maximum_batch
    source_policy::volcano::tests::performance_gate_volcano_maximum_selection
  )
  if [[ "$*" == 'test --release --locked --bin quota-throttle performance_gate_ -- --ignored --list' ]]; then
    case "$PERFORMANCE_FIXTURE_MODE" in
      list_failure) return 19 ;;
      empty) return 0 ;;
      missing) unset 'tests[3]' ;;
      extra) tests+=(unexpected::performance_gate_extra) ;;
      duplicate) tests+=("${tests[0]}") ;;
      happy|run_failure) ;;
      *) return 47 ;;
    esac
    printf '%s: test\n' "${tests[@]}"
    printf '\n%s tests, 0 benchmarks\n' "${#tests[@]}"
  elif [[ "$*" == 'test --release --locked --bin quota-throttle performance_gate_ -- --ignored --test-threads=1 --nocapture' ]]; then
    [[ "$PERFORMANCE_FIXTURE_MODE" != run_failure ]] || return 17
  else
    return 46 # 不能通过改过滤条件、删 ignored 或串行参数使 mock 蒙混过去。
  fi
}
export -f cargo

for entry in happy:0 empty:1 missing:1 extra:1 duplicate:1 list_failure:19 run_failure:17; do
  mode="${entry%:*}"
  expected_status="${entry#*:}"
  status=0
  PERFORMANCE_FIXTURE_MODE="$mode" bash scripts/tests/performance-gates.sh >/dev/null 2>&1 || status=$?
  if [[ "$status" != "$expected_status" ]]; then
    printf 'Runner regression failed: %s expected=%s actual=%s\n' "$mode" "$expected_status" "$status" >&2
    exit 1
  fi
done
status=0
PERFORMANCE_FIXTURE_MODE=happy bash scripts/tests/performance-gates.sh --test-threads=64 >/dev/null 2>&1 || status=$?
test "$status" -eq 2
printf 'Performance runner regressions: 8 passed\n'
