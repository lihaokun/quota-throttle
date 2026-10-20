#!/usr/bin/env bash
# 仅 mock cargo 的列表和退出码，不编译、不创建夹具文件、不访问网络。
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."

cargo() {
  local tests=(
    catalog::store::tests::control::performance_gate_control_group_priorities
    panel_http::publication::tests::capacity::performance_gate_panel_http_encoding
    panel_http::publication::tests::capacity::performance_gate_panel_http_body
    panel_http::admin::tests::performance_gate_admin_request_decode
    panel_http::admin::output::tests::performance_gate_admin_usage_encoding
    catalog::store::tests::performance_gate_import_and_snapshot
    catalog::store::tests::execution::performance_gate_group_work_snapshot
    source_policy::tests::performance_gate_glm_pure_capacity
    catalog::store::tests::panel::performance_gate_panel_maximum_catalog
    catalog::store::tests::glm_round::performance_gate_glm_maximum_catalog
  catalog::store::tests::tokens::migration::performance_gate_token_identity_10k
  catalog::store::tests::tokens::migration::performance_gate_token_v7_migration_10k_operations
  catalog::store::tests::tokens::service::performance_gate_group_references_10k
  newapi::groups::tests::performance_gate_group_options_10k
    catalog::store::tests::policy_commands::migration::performance_gate_c2_v6_migration_10k_operations
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
    newapi::tokens::tests::performance_gate_token_maximum_page
    source_policy::volcano::tests::performance_gate_volcano_maximum_selection
  )
  if [[ "$*" == 'test --release --locked --bin quota-throttle performance_gate_ -- --ignored --list' ]]; then
    case "$PERFORMANCE_FIXTURE_MODE" in
      list_failure) return 19 ;;
      empty) return 0 ;;
      missing) unset 'tests[3]' ;;
      extra) tests+=(unexpected::performance_gate_extra) ;;
      duplicate) tests+=("${tests[0]}") ;;
      happy|run_failure|debug_empty|debug_wrong|debug_list_failure|debug_run_failure|import_debug_empty|import_debug_wrong|import_debug_list_failure|import_debug_run_failure|journal_debug_empty|journal_debug_wrong|journal_debug_list_failure|journal_debug_run_failure|glm_debug_empty|glm_debug_wrong|glm_debug_list_failure|glm_debug_run_failure) ;;
      *) return 47 ;;
    esac
    printf '%s: test\n' "${tests[@]}"
    printf '\n%s tests, 0 benchmarks\n' "${#tests[@]}"
  elif [[ "$*" == 'test --release --locked --bin quota-throttle performance_gate_ -- --ignored --test-threads=1 --nocapture' ]]; then
    [[ "$PERFORMANCE_FIXTURE_MODE" != run_failure ]] || return 17
  elif [[ "$*" == 'test --locked --bin quota-throttle catalog::store::tests::control::performance_gate_control_group_priorities -- --ignored --exact --list' ]]; then
    case "$PERFORMANCE_FIXTURE_MODE" in
      debug_empty) return 0 ;;
      debug_wrong) printf 'wrong::performance_gate_control_group_priorities: test\n' ;;
      debug_list_failure) return 23 ;;
      *) printf 'catalog::store::tests::control::performance_gate_control_group_priorities: test\n' ;;
    esac
  elif [[ "$*" == 'test --locked --bin quota-throttle catalog::store::tests::control::performance_gate_control_group_priorities -- --ignored --exact --test-threads=1 --nocapture' ]]; then
    [[ "$PERFORMANCE_FIXTURE_MODE" != debug_run_failure ]] || return 29
  elif [[ "$*" == 'test --locked --bin quota-throttle catalog::store::tests::performance_gate_import_and_snapshot -- --ignored --exact --list' ]]; then
    case "$PERFORMANCE_FIXTURE_MODE" in
      import_debug_empty) return 0 ;;
      import_debug_wrong) printf 'wrong::performance_gate_import_and_snapshot: test\n' ;;
      import_debug_list_failure) return 31 ;;
      *) printf 'catalog::store::tests::performance_gate_import_and_snapshot: test\n' ;;
    esac
  elif [[ "$*" == 'test --locked --bin quota-throttle catalog::store::tests::performance_gate_import_and_snapshot -- --ignored --exact --test-threads=1 --nocapture' ]]; then
    [[ "$PERFORMANCE_FIXTURE_MODE" != import_debug_run_failure ]] || return 37
  elif [[ "$*" == 'test --locked --bin quota-throttle catalog::store::tests::execution::performance_gate_group_work_snapshot -- --ignored --exact --list' ]]; then
    case "$PERFORMANCE_FIXTURE_MODE" in
      journal_debug_empty) return 0 ;;
      journal_debug_wrong) printf 'wrong::performance_gate_group_work_snapshot: test\n' ;;
      journal_debug_list_failure) return 41 ;;
      *) printf 'catalog::store::tests::execution::performance_gate_group_work_snapshot: test\n' ;;
    esac
  elif [[ "$*" == 'test --locked --bin quota-throttle catalog::store::tests::execution::performance_gate_group_work_snapshot -- --ignored --exact --test-threads=1 --nocapture' ]]; then
    [[ "$PERFORMANCE_FIXTURE_MODE" != journal_debug_run_failure ]] || return 43
  elif [[ "$*" == 'test --locked --bin quota-throttle source_policy::tests::performance_gate_glm_pure_capacity -- --ignored --exact --list' ]]; then
    case "$PERFORMANCE_FIXTURE_MODE" in
      glm_debug_empty) return 0 ;;
      glm_debug_wrong) printf 'wrong::performance_gate_glm_pure_capacity: test\n' ;;
      glm_debug_list_failure) return 53 ;;
      *) printf 'source_policy::tests::performance_gate_glm_pure_capacity: test\n' ;;
    esac
  elif [[ "$*" == 'test --locked --bin quota-throttle source_policy::tests::performance_gate_glm_pure_capacity -- --ignored --exact --test-threads=1 --nocapture' ]]; then
    [[ "$PERFORMANCE_FIXTURE_MODE" != glm_debug_run_failure ]] || return 59
  else
    return 46 # 不能通过改过滤条件、删 ignored 或串行参数使 mock 蒙混过去。
  fi
}
export -f cargo

for entry in happy:0 empty:1 missing:1 extra:1 duplicate:1 list_failure:19 run_failure:17 debug_empty:1 debug_wrong:1 debug_list_failure:23 debug_run_failure:29 import_debug_empty:1 import_debug_wrong:1 import_debug_list_failure:31 import_debug_run_failure:37 journal_debug_empty:1 journal_debug_wrong:1 journal_debug_list_failure:41 journal_debug_run_failure:43 glm_debug_empty:1 glm_debug_wrong:1 glm_debug_list_failure:53 glm_debug_run_failure:59; do
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
printf 'Performance runner regressions: 24 passed\n'
