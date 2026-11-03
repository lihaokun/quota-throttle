#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=80ba157
git diff --check "$base"
while IFS= read -r path; do
  case "$path" in
    src/main.rs|src/panel_http.rs|src/panel_http/*|scripts/contract_audit/sgm6_a21.sh|scripts/tests/performance-gates.sh|scripts/tests/performance-gates-regression.sh|docs/design/source-groups-token-management/*|docs/audits/sgm-6-three-page-panel/*|docs/fixes/panel-http-fix-ready-deadline.md|proptest-regressions/panel_http/*) ;;
    *) printf 'Unexpected A2.1 path: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <({ git diff --name-only "$base"; git ls-files --others --exclude-standard; } | LC_ALL=C sort -u)
git diff --exit-code "$base" -- Cargo.toml Cargo.lock src ':(exclude)src/main.rs' ':(exclude)src/panel_http.rs' ':(exclude)src/panel_http'
git diff --exit-code "$base" -- docs/design/source-groups-token-management/schemas/panel-snapshot.schema.json docs/design/source-groups-token-management/schemas/panel-http-error.schema.json
diff -u <(git show "$base:src/main.rs") <(sed '/^mod panel_http;$/d' src/main.rs)
for file in scripts/tests/performance-gates.sh scripts/tests/performance-gates-regression.sh; do
  test "$(rg -c 'panel_http::publication::tests::capacity::performance_gate_panel_http_' "$file")" -eq 2
  diff -u <(git show "$base:$file") <(sed -E '/^ +panel_http::publication::tests::capacity::performance_gate_panel_http_(encoding|body)( \\)?$/d' "$file")
done
for step in H1.1 H1.2 H1.3 H1.4 H1.5 H1.6 H1.7; do
  test "$(rg -F "// Step $step:" src/panel_http.rs src/panel_http/request.rs src/panel_http/publication.rs | wc -l)" -eq 1
done
if rg -n 'NewApiClient|CatalogStore|send_management|read_source_material|source_secrets|reconcile_once|refresh_glm|refresh_volcano|ControlCommand' src/panel_http.rs src/panel_http/request.rs src/panel_http/publication.rs; then
  printf 'Unexpected business/raw-material path in HTTP boundary\n' >&2; exit 1
fi
test "$(rg -F -c '.panel_snapshot()' src/panel_http.rs)" -eq 1
rustfmt --edition 2021 --config skip_children=true --check src/panel_http.rs src/panel_http/request.rs src/panel_http/publication.rs src/panel_http/tests.rs src/panel_http/tests/request.rs src/panel_http/tests/publication.rs src/panel_http/tests/capacity.rs
bash scripts/tests/performance-gates-regression.sh
r=panel_http::request::tests
p=panel_http::publication::tests
c=$p::capacity
w=panel_http::tests
expected="$(printf '%s\n' \
 "$r::h1_origin_and_authority_matrix_never_trusts_request_aliases" \
 "$r::h1_strict_head_rejects_ambiguity_invalid_lengths_and_control_bytes" \
 "$r::h1_head_target_and_field_limits_include_exact_boundaries" \
 "$r::h1_oversized_delimiter_in_same_read_cannot_bypass_limit" \
 "$r::h1_origin_csrf_fetch_and_json_guards_are_independent" \
 "$r::h1_slow_head_body_eof_and_utf8_fail_without_partial_success" \
 "$r::h1_pb1_single_message_framing_is_partition_invariant" \
 "$r::h1_pb2_untrusted_header_combinations_never_authorize" \
 "$r::h1_ready_head_cannot_succeed_at_absolute_deadline" \
 "$r::h1_ready_or_buffered_body_cannot_succeed_at_absolute_deadline" \
 "$r::h1_read_returning_ready_at_deadline_does_not_become_success" \
 "$r::h1_body_deadline_is_anchored_at_head_completion_including_buffered_body" \
 "$p::h1_cache_age_bounds_round_up_and_never_revive_after_clock_reversal" \
 "$p::h1_cache_empty_busy_poison_and_repeated_reads_do_not_mutate_evidence" \
 "$p::h1_bounded_writer_rejects_before_append_and_limits_capacity_growth" \
 "$p::h1_publication_completion_never_renews_start_or_accepts_late_results" \
 "$p::h1_pb3_cache_reference_trace_never_renews_or_revives" \
 "$c::h1_maximum_valid_domain_fits_each_published_capacity_budget" \
 "$c::h1_field_width_bounds_cover_optional_states_numbers_and_all_shared_enums" \
 "$c::h1_oversized_encoding_and_error_wire_never_publish_private_shapes" \
 "$c::h1_maximum_body_keeps_all_bytes_without_timing_gate" \
 "$c::h1_pb4_safe_encoding_preserves_input_unicode_and_complete_arrays" \
 "$c::performance_gate_panel_http_encoding" \
 "$c::performance_gate_panel_http_body" \
 "$w::h1_bind_rejects_nonloopback_and_canonicalizes_fixed_origin" \
 "$w::h1_http_state_uses_exact_bytes_headers_and_one_request_only" \
 "$w::h1_http_rejections_are_safe_and_head_never_has_body" \
 "$w::h1_response_stalled_writer_has_one_absolute_deadline" \
 "$w::h1_ready_writer_must_not_start_after_connection_deadline" \
 "$w::h1_ready_write_and_flush_recheck_deadline_before_reporting_completion" \
 "$w::h1_unified_publication_is_not_request_driven_and_failure_clears_cache" \
 "$w::h1_expired_blocking_publication_keeps_ownership_and_shutdown_drains" \
 "$w::h1_blocking_panic_and_preclosed_shutdown_are_fail_closed" \
 "$w::h1_connection_limit_closes_excess_and_recovers_without_queue" \
 "$w::h1_real_worker_publication_keeps_private_source_material_outside_http" \
 | LC_ALL=C sort)"
expected_ignored="$(printf '%s\n' "$c::performance_gate_panel_http_encoding" "$c::performance_gate_panel_http_body" | LC_ALL=C sort)"
test "$(printf '%s\n' "$expected" | wc -l)" -eq 35
manifest_ok() { [[ -n "$1" && "$1" == "$expected" && "$2" == "$expected_ignored" && "$1" != "$2" ]]; }
if manifest_ok '' '' || manifest_ok "$expected" "$expected" || manifest_ok "${expected#*$'\n'}" "$expected_ignored"; then exit 1; fi
listing="$(cargo test --locked --offline -j 2 -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^panel_http::' | LC_ALL=C sort || true)"
listing="$(cargo test --locked --offline -j 2 -- --ignored --list)"
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^panel_http::' | LC_ALL=C sort || true)"
manifest_ok "$actual" "$ignored" || { printf 'A2.1 exact manifest mismatch\n' >&2; exit 1; }
test "$(printf '%s\n' "$actual" | rg -c '::h1_pb[1-4]_')" -eq 4
output="$(cargo test --locked --offline -j 2 "$w::h1_real_worker_publication_keeps_private_source_material_outside_http" -- --exact --nocapture)"
snapshot="$(printf '%s\n' "$output" | sed -n 's/^.*HTTP_PANEL_JSON=//p')"
test -n "$snapshot"
check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/panel-snapshot.schema.json --force-filetype json <(printf '%s\n' "$snapshot")
output="$(cargo test --locked --offline -j 2 "$c::h1_oversized_encoding_and_error_wire_never_publish_private_shapes" -- --exact --nocapture)"
errors="$(printf '%s\n' "$output" | sed -n 's/^.*HTTP_ERRORS_JSON=//p')"
test "$(printf '%s\n' "$errors" | jq 'length')" -eq 15
schema=docs/design/source-groups-token-management/schemas/panel-http-error.schema.json
test "$(printf '%s\n' "$errors" | jq -c '[.[].body.error]|sort')" == "$(jq -c '.properties.error.enum|sort' "$schema")"
while IFS= read -r body; do
  check-jsonschema --schemafile "$schema" --force-filetype json <(printf '%s\n' "$body")
done < <(printf '%s\n' "$errors" | jq -c '.[].body')
printf '%s\n' "$errors" | jq -e '
 {bad_request:400,header_too_large:431,body_too_large:413,target_too_long:414,length_required:411,unsupported_media_type:415,expectation_failed:417,unsupported_version:505,forbidden:403,not_found:404,method_not_allowed:405,overloaded:503,request_timeout:408,snapshot_unavailable:503,internal:500} as $codes
 | all(.[]; (.status[:3]|tonumber) == $codes[.body.error])' >/dev/null
for body in '{"schema_version":1,"error":"unknown"}' '{"schema_version":2,"error":"bad_request"}' '{"schema_version":1,"error":null}' '{"schema_version":1,"error":"bad_request","credential":"fake"}' '{"error":"bad_request"}' '{"schema_version":1,"error":{"code":"bad_request"}}'; do
  if check-jsonschema --schemafile "$schema" --force-filetype json <(printf '%s\n' "$body") >/dev/null 2>&1; then exit 1; fi
done
for change in '.credential="fake"' '.catalog.sources[0].source.secret_ref="fake"' '.captured_at_ms=-1'; do
  bad="$(printf '%s\n' "$snapshot" | jq -c "$change")"
  if check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/panel-snapshot.schema.json --force-filetype json <(printf '%s\n' "$bad") >/dev/null 2>&1; then exit 1; fi
done
printf 'SGM6 A2.1 frozen core, schemas/statuses, seven Steps, exact 35 entries and 24 runner checks passed; full/mutation/performance remain separate.\n'
