#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=999ca41
git diff --check "$base"
while IFS= read -r path; do
  case "$path" in
    src/catalog/panel.rs|src/catalog/panel_tests.rs|src/catalog/mod.rs|src/catalog/tests.rs|src/catalog/glm_publication.rs|src/reconcile/worker.rs|src/reconcile/worker/policy.rs|src/reconcile/worker/panel.rs|src/reconcile/worker/panel_tests.rs|src/reconcile/worker/tests.rs|docs/design/source-groups-token-management/*|docs/audits/sgm-6-three-page-panel/*|docs/fixes/panel-state-fix-retired-stamp-scope.md|docs/fixes/catalog-fix-import-performance-gate.md|scripts/contract_audit/sgm6_a1.sh|scripts/tests/performance-gates.sh|scripts/tests/performance-gates-regression.sh|proptest-regressions/catalog/panel_tests.txt|proptest-regressions/reconcile/worker/panel_tests.txt) ;;
    src/catalog/execution_tests.rs|src/source_policy/tests.rs) ;; # 已审核的纯容量门禁等价提取
    *) printf 'Unexpected SGM6 A1 path: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <({ git diff --name-only "$base"; git ls-files --others --exclude-standard; } | LC_ALL=C sort -u)
git diff --exit-code "$base" -- Cargo.toml Cargo.lock src/main.rs src/boot.rs src/config.rs src/status.rs src/orchestrator.rs src/quota.rs src/source_policy src/ledger src/catalog/schema_v1.sql src/catalog/schema_v2.sql src/catalog/schema_v3.sql src/catalog/schema_v4.sql src/catalog/schema_v5.sql src/catalog/schema_v6.sql src/catalog/schema_v7.sql src/catalog/schema_v8.sql src/catalog/store.rs src/catalog/control.rs src/catalog/confirmation.rs src/catalog/execution.rs src/catalog/policy.rs src/catalog/policy_commands.rs src/catalog/glm_round.rs src/catalog/volcano_publication.rs src/catalog/ledger.rs src/catalog/ledger src/catalog/tokens.rs src/catalog/tokens src/newapi.rs src/newapi src/reconcile/planner.rs src/reconcile/planner src/reconcile/worker/analytics.rs src/reconcile/worker/analytics src/reconcile/worker/collection.rs src/reconcile/worker/collection src/reconcile/worker/management.rs src/reconcile/worker/token_service.rs src/reconcile/worker/token_service ':(exclude)src/source_policy/tests.rs'
diff -u <(git show "$base:src/catalog/mod.rs") <(sed '/^pub(crate) mod panel;$/d' src/catalog/mod.rs)
expected_import_wrappers='fn import_and_snapshot_scale_within_budget() {
    import_and_snapshot_capacity_case();
}

#[test]
#[ignore = "独立串行性能门禁：bash scripts/tests/performance-gates.sh"]
fn performance_gate_import_and_snapshot() {
    let (import_elapsed, snapshot_elapsed) = import_and_snapshot_capacity_case();
    println!(
        "IMPORT_GATE_MS={} SNAPSHOT_GATE_MS={}",
        import_elapsed.as_millis(),
        snapshot_elapsed.as_millis()
    );
    assert!(import_elapsed < Duration::from_secs(10));
    assert!(snapshot_elapsed < Duration::from_secs(10));
}

fn import_and_snapshot_capacity_case() -> (Duration, Duration) {'
test "$(sed -n '/^fn import_and_snapshot_scale_within_budget()/,/^fn import_and_snapshot_capacity_case()/p' src/catalog/tests.rs)" == "$expected_import_wrappers"
diff -u <(git show "$base:src/catalog/tests.rs") <(sed '/^#\[path = "panel_tests.rs"\]$/,+1d' src/catalog/tests.rs | awk '
  /^fn import_and_snapshot_scale_within_budget\(\)/ {print;skip=1;next}
  /^fn import_and_snapshot_capacity_case\(\)/ {skip=0;next}
  skip {next}
  /^    let (import|snapshot)_elapsed = start.elapsed\(\);$/ {print "    assert!(start.elapsed() < Duration::from_secs(10));";next}
  /^    \(import_elapsed, snapshot_elapsed\)$/ {next}
  {print}
')
check_single_capacity() {
  local path=$1 original=$2 case_name=$3 gate=$4 budget=$5 metric=$6 timer=$7 mode=$8 expected statement
  printf -v expected 'fn %s() {\n    %s();\n}\n\n#[test]\n#[ignore = "独立串行性能门禁：bash scripts/tests/performance-gates.sh"]\nfn %s() {\n    let elapsed = %s();\n    println!("%s={}", elapsed.as_millis());\n    assert!(elapsed < Duration::from_secs(%s));\n}\n\nfn %s() -> Duration {' "$original" "$case_name" "$gate" "$case_name" "$metric" "$budget" "$case_name"
  test "$(sed -n "/^fn $original()/,/^fn $case_name()/p" "$path")" == "$expected"
  statement="    let elapsed = $timer.elapsed();"
  if [[ "$mode" == direct ]]; then statement="    $timer.elapsed()"; fi
  diff -u <(git show "$base:$path") <(awk -v original="$original" -v name="$case_name" -v statement="$statement" -v timer="$timer" -v budget="$budget" '
    $0 == "fn " original "() {" {print;skip=1;next}
    $0 == "fn " name "() -> Duration {" {skip=0;incase=1;next}
    skip {next}
    incase && $0 == statement {print "    assert!(" timer ".elapsed() < Duration::from_secs(" budget "));";next}
    incase && $0 == "    elapsed" {next}
    incase && $0 == "}" {incase=0}
    {print}
  ' "$path")
}
check_single_capacity src/catalog/execution_tests.rs journal_group_snapshot_has_bounded_cost group_work_snapshot_case performance_gate_group_work_snapshot 3 GROUP_WORK_SNAPSHOT_MS started direct
check_single_capacity src/source_policy/tests.rs capacity_bounds_and_maximum_snapshot_are_finite glm_pure_capacity_case performance_gate_glm_pure_capacity 5 GLM_PURE_CAPACITY_MS start captured
diff -u <(git show "$base:src/reconcile/worker/tests.rs") <(sed '/^#\[path = "panel_tests.rs"\]$/,+1d' src/reconcile/worker/tests.rs)
diff -u <(git show "$base:src/catalog/glm_publication.rs") <(awk '
  /^    pub assessments: Vec<super::panel::CommittedGroupAssessment>,$/ {next}
  /^            assessments: applied$/ {skip=1;next}
  skip && /^                \.collect\(\),$/ {skip=0;next}
  !skip {print}
' src/catalog/glm_publication.rs)
diff -u <(git show "$base:src/reconcile/worker.rs") <(sed '/^pub(crate) mod panel;$/d; /^    panel_glm:/d; /^                panel_glm: None,$/d' src/reconcile/worker.rs)
diff -u <(git show "$base:src/reconcile/worker/policy.rs") <(awk '
  /^pub\(super\) fn current_ms/ {sub("pub\\(super\\) ","")}
  /\/\/ Step A1.3:/ {next}
  /^            c.panel_glm = Some/ {skip=1;next}
  skip && /^            \)\);$/ {skip=0;next}
  !skip {print}
' src/reconcile/worker/policy.rs)
for step in A1.1 A1.2 A1.3 A1.4 A1.5; do
  test "$(rg -F "// Step $step:" src/catalog/panel.rs src/reconcile/worker/panel.rs src/reconcile/worker/policy.rs | wc -l)" -eq 1
done
rg -q '^pub\(crate\) const PANEL_SCHEMA_VERSION: i64 = 1;' src/reconcile/worker/panel.rs
if rg -n 'source_secrets|execution_attempts|pending_writes|read_material|send_management|\.execute\(' src/catalog/panel.rs src/reconcile/worker/panel.rs; then
  printf 'Forbidden data/write path in panel producer\n' >&2; exit 1
fi
rustfmt --edition 2021 --config skip_children=true --check src/catalog/panel.rs src/catalog/panel_tests.rs src/catalog/glm_publication.rs src/reconcile/worker/panel.rs src/reconcile/worker/panel_tests.rs src/reconcile/worker/policy.rs
bash scripts/tests/performance-gates-regression.sh
c=catalog::store::tests::panel
w=reconcile::worker::tests::panel_tests
expected="$(printf '%s\n' \
 "$c::s6a1_catalog_empty_and_private_types" \
 "$c::s6a1_catalog_schema_is_whitelisted_and_preserves_runtime" \
 "$c::s6a1_catalog_missing_or_corrupt_rows_fail_without_writes" \
 "$c::s6a1_catalog_retire_intent_and_archived_identity_are_not_success_labels" \
 "$c::s6a1_pb1_catalog_projection_matches_input_model" \
 "$c::s6a1_catalog_maximum_fixture_preserves_all_rows" \
 "$c::performance_gate_panel_maximum_catalog" \
 "$w::s6a1_snapshot_is_readonly_unknown_and_provider_scoped" \
 "$w::s6a1_clock_busy_and_poison_have_distinct_safe_errors" \
 "$w::s6a1_observed_time_boundaries_do_not_renew_from_completion" \
 "$w::s6a1_empty_failed_and_changed_during_probe_have_bounded_assessments" \
 "$w::s6a1_invalid_display_fields_and_large_cycles_remain_safe" \
 "$w::s6a1_retirement_missing_members_and_new_binding_invalidate_display" \
 "$w::s6a1_failed_commit_replay_and_preview_keep_prior_display" \
 "$w::s6a1_imminent_reset_and_pin_release_are_last_committed_events" \
 "$w::s6a1_reopen_preserves_catalog_but_not_observation_cache" \
 "$w::s6a1_cancelled_real_probe_does_not_replace_committed_display" \
 "$w::s6a1_pb2_quota_state_matches_independent_input_model" \
 "$w::s6a1_pb3_group_stamp_covers_all_inputs_not_observation_fields" | LC_ALL=C sort)"
expected_ignored="$c::performance_gate_panel_maximum_catalog"
test "$(printf '%s\n' "$expected" | wc -l)" -eq 19
manifest_ok() { [[ -n "$1" && "$1" == "$expected" && "$2" == "$expected_ignored" && "$1" != "$2" ]]; }
if manifest_ok '' '' || manifest_ok "$expected" "$expected" || manifest_ok "${expected#*$'\n'}" "$expected_ignored"; then exit 1; fi
listing="$(cargo test --locked --offline -j 2 -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg "^($c|$w)::" | LC_ALL=C sort || true)"
listing="$(cargo test --locked --offline -j 2 -- --ignored --list)"
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg "^($c|$w)::" | LC_ALL=C sort || true)"
manifest_ok "$actual" "$ignored" || { printf 'SGM6 A1 manifest mismatch\n' >&2; exit 1; }
test "$(printf '%s\n' "$actual" | rg -c '::s6a1_pb[1-3]_')" -eq 3
output="$(cargo test --locked --offline -j 2 "$c::s6a1_catalog_schema_is_whitelisted_and_preserves_runtime" -- --exact --nocapture)"
catalog="$(printf '%s\n' "$output" | sed -n 's/^PANEL_SCHEMA_JSON=//p')"
enums="$(printf '%s\n' "$output" | sed -n 's/^PANEL_ENUM_JSON=//p')"
output="$(cargo test --locked --offline -j 2 "$w::s6a1_imminent_reset_and_pin_release_are_last_committed_events" -- --exact --nocapture)"
observed="$(printf '%s\n' "$output" | sed -n 's/^PANEL_SCHEMA_JSON=//p')"
output="$(cargo test --locked --offline -j 2 "$w::s6a1_invalid_display_fields_and_large_cycles_remain_safe" -- --exact --nocapture)"
large="$(printf '%s\n' "$output" | sed -n 's/^PANEL_SCHEMA_JSON=//p')"
schema=docs/design/source-groups-token-management/schemas/panel-snapshot.schema.json
for snapshot in "$catalog" "$observed" "$large"; do
  test -n "$snapshot"
  check-jsonschema --schemafile "$schema" --force-filetype json <(printf '%s\n' "$snapshot")
done
for key in id kind name revision priority timestamp source group membership; do
  diff -u <(jq -S --arg key "$key" '."$defs"[$key]' "$schema") <(jq -S --arg key "$key" '."$defs"[$key]' docs/design/source-groups-token-management/schemas/catalog-snapshot.schema.json)
done
for pair in 'state quota.properties.state' 'status_directive panel_membership.properties.status_directive' 'create_state panel_membership.properties.create_state' 'error_code control_error'; do
  key=${pair%% *}; path=${pair#* }
  test "$(printf '%s\n' "$enums" | jq -c --arg key "$key" '.[$key]|sort')" == "$(jq -c '."$defs".'"$path"'.enum|map(select(.!=null))|sort' "$schema")"
done
for expression in '.credential="plaintext"' '.catalog.sources[0].source.secret_ref="x"' '.catalog.memberships[0].create_name="x"' '.catalog.sources[0].volcano=.catalog.sources[1].volcano' '.catalog.groups[1].pinned_member_id="1"' '.catalog.sources[1].volcano.runtime.used_input=-1'; do
  bad="$(printf '%s\n' "$catalog" | jq -c "$expression")"
  if check-jsonschema --schemafile "$schema" --force-filetype json <(printf '%s\n' "$bad") >/dev/null 2>&1; then exit 1; fi
done
for expression in '.glm.sources[0].level="   "' '.glm.sources[0].level="　"' '.glm.sources[0].five_hour.percentage=-1' '.glm.groups[0].current=false' '.glm.groups[0].cycle_id=null' '.glm.sources[0].source_revision=0'; do
  bad="$(printf '%s\n' "$observed" | jq -c "$expression")"
  if check-jsonschema --schemafile "$schema" --force-filetype json <(printf '%s\n' "$bad") >/dev/null 2>&1; then exit 1; fi
done
printf 'SGM6 A1 frozen core, schema/enums, five Steps and exact 19-entry manifest passed; full regression/mutation/performance are separate.\n'
