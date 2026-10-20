#!/usr/bin/env bash
# Only mutate a private copy. A compiler error or zero-test run is never a catch.
set -euo pipefail
baseline=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
audit_dir=$(mktemp -d /tmp/quota-throttle-sgm6-a22-mutations.XXXXXX)
printf 'Mutation workspace: %s\n' "$audit_dir"
cp -a "$baseline/src" "$baseline/Cargo.toml" "$baseline/Cargo.lock" "$baseline/config.example.toml" "$baseline/proptest-regressions" "$audit_dir/"
cp -a --reflink=auto "$baseline/target" "$audit_dir/target"
cd "$audit_dir"
export CARGO_TARGET_DIR="$audit_dir/target" CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=2
files=(src/panel_http.rs src/panel_http/admin/{bridge.rs,input.rs,route.rs,dispatch.rs,output/usage.rs,input/source.rs} src/reconcile/worker/token_service/execution.rs)
restore() {
  local file
  for file in "${files[@]}"; do cp "$baseline/$file" "$file"; touch "$file"; done
}
trap restore EXIT
replace() {
  BEFORE="$2" AFTER="$3" perl -0777 -i -pe '
    BEGIN { $a=$ENV{BEFORE}; $b=$ENV{AFTER}; }
    $n=()=/\Q$a\E/g; die "expected exactly one mutation match, got $n\n" if $n != 1;
    s/\Q$a\E/$b/;
  ' "$1"
}
run_case() {
  local label=$1 test_name=$2 status
  set +e
  timeout 420 nice -n 10 cargo test --locked --offline "$test_name" -- --exact --nocapture >"$audit_dir/$label.log" 2>&1
  status=$?
  set -e
  [[ "$status" == 101 ]]
  rg -q 'Compiling quota-throttle' "$audit_dir/$label.log"
  rg -q '^test result: FAILED\. 0 passed; 1 failed; 0 ignored;' "$audit_dir/$label.log"
  ! rg -q '^error\[|could not compile|test result: ok' "$audit_dir/$label.log"
  ! rg -q 'Operation not permitted|PermissionDenied|BindFailed|No space left on device|Too many open files' "$audit_dir/$label.log"
  # 这里只证明有效单失败；主执行者与独审还须核对每案断言/左右值确属目标契约。
  printf '%s caught: compiled, exact one test failed\n' "$label"
  restore
}
b=panel_http::admin::bridge::tests
t=panel_http::admin::tests
h=$t::http
o=panel_http::admin::output::tests
k=reconcile::worker::tests::token_service_tests::panel_admin
restore
replace src/panel_http/admin/bridge.rs 'if self.dry_run && request.writes()' 'if false && self.dry_run && request.writes()'
run_case m01_missing_dry_run "$b::h2_closing_discards_handoff_and_dry_run_rejects_before_admission"
replace src/panel_http/admin/input.rs 'expected_policy_revision: revision(d.expected_policy_revision)?,' 'expected_policy_revision: revision(d.expected_revision)?,'
run_case m02_wrong_revision_category "$t::h2_command_mapping_preserves_targets_revisions_and_intent"
replace src/reconcile/worker/token_service/execution.rs 'self.token_client(TokenOwnerScope::Issuer)?.1' 'self.token_client(TokenOwnerScope::LegacyAdmin)?.1'
run_case m03_create_under_nonissuer "$k::h2_tcp_token_create_native_local_reveal_enable_delete_use_original_service"
replace src/panel_http/admin/dispatch.rs 'secret: worker.reveal_token(token, revision).await?,' 'secret: worker.reveal_token(crate::catalog::ManagedTokenId::new(worker.token_identity(token)?.native_token_id.unwrap().parse().unwrap()).unwrap(), revision).await?,'
run_case m04_native_id_as_local "$k::h2_tcp_token_create_native_local_reveal_enable_delete_use_original_service"
replace src/panel_http/admin/input.rs $'    let mut parser = serde_json::Deserializer::from_slice(bytes);\n    let value = object(&mut parser).map_err(|_| bad_request())?;\n    parser.end().map_err(|_| bad_request())?;' $'    let raw: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| bad_request())?;\n    let value = serde_json::from_value(raw).map_err(|_| bad_request())?;'
run_case m05_duplicate_json_overwrite "$t::h2_json_duplicates_required_null_and_privileged_fields_are_rejected"
replace src/panel_http/admin/route.rs '|| values.insert(k, v).is_some()' '|| { values.insert(k, v); false }'
run_case m06_duplicate_query_overwrite "$t::h2_query_and_id_boundaries_do_not_alias"
replace src/panel_http/admin/bridge.rs $'        if Instant::now() >= deadline {\n            return unconfirmed();\n        }\n        match result' $'        if Instant::now() >= deadline {\n            self.slots.add_permits(1);\n            return unconfirmed();\n        }\n        match result'
run_case m07_timeout_releases_actual_slot "$b::h2_timeouts_do_not_release_actual_jobs_or_forge_rollback"
replace src/panel_http.rs '        while !business.jobs.is_empty() {' $'        business.jobs.detach_all();\n        while !business.jobs.is_empty() {'
run_case m08_shutdown_detaches_jobs "$b::h2_shutdown_cannot_detach_business_even_when_publisher_never_blocks"
replace src/panel_http.rs '                        business.call(&head, body, connection_end).await' $'                        let response = business.call(&head, body, connection_end).await;\n                        let _ = cache.replace(Some(publication::PublishedPanel { bytes: response.body.clone(), captured_at_ms: wall_ms().unwrap(), started_at: Instant::now() }));\n                        response'
run_case m09_reveal_in_ordinary_cache "$k::h2_tcp_token_create_native_local_reveal_enable_delete_use_original_service"
replace src/panel_http/admin/dispatch.rs 'Ok(models) => (models, ModelOrigin::Discovered),' 'Ok(mut models) => { if let ModelSelection::DiscoverOrManual(manual) = selection { models.extend(manual); } (models, ModelOrigin::Discovered) },'
run_case m10_discovery_merges_fallback "$h::h2_tcp_model_discovery_is_authoritative_and_duplicate_precedes_network"
replace src/panel_http/admin/output/usage.rs 'usage: snapshot.usage.as_ref().map(usage).transpose()?,' 'usage: snapshot.usage.as_ref().map(usage).transpose()?.or(Some(Usage { total: Totals { input: 0, output: 0, total: 0, consumption_rows: 0 }, models: vec![], tokens: vec![], buckets: vec![] })),'
run_case m11_unknown_usage_filled_zero "$o::h2_usage_uninitialized_and_unknown_filters_remain_distinct"
replace src/panel_http/admin/dispatch.rs 'R::Retry(id) => AdminReply::Operation(worker.retry(&id).await?),' 'R::Retry(id) => AdminReply::Operation(worker.retry(&crate::catalog::control::OperationId::new(format!("{}_new", id.as_str())).unwrap()).await?),'
run_case m12_retry_changes_operation_id "$k::h2_tcp_token_create_native_local_reveal_enable_delete_use_original_service"
replace src/panel_http/admin/input.rs 'd.deserialize_map(ObjectVisitor(std::marker::PhantomData))' 'T::deserialize(d)'
run_case m13_struct_arrays_accepted "$t::h2_serde_top_level_arrays_are_not_objects"
replace src/panel_http/admin/input.rs $'    T::deserialize(serde::de::value::StringDeserializer::<D::Error>::new(\n        String::deserialize(d)?,\n    ))' '    T::deserialize(d)'
run_case m14_scalar_enum_objects_accepted "$t::h2_serde_scalar_enums_require_strings"
replace src/panel_http/admin/input/source.rs '    Discover {},' '    Discover,'
replace src/panel_http/admin/input/source.rs '        Self::Discover {}' '        Self::Discover'
replace src/panel_http/admin/input/source.rs '        Selection::Discover {} => ModelSelection::Discover,' '        Selection::Discover => ModelSelection::Discover,'
run_case m15_unit_variants_ignore_extra_fields "$t::h2_unit_model_mode_rejects_unknown_fields"
replace src/panel_http/admin/bridge.rs $'        if Instant::now() >= deadline {\n            return unconfirmed();\n        }\n        match result' $'        if false && Instant::now() >= deadline {\n            return unconfirmed();\n        }\n        match result'
run_case m16_ready_reply_ignores_absolute_deadline "$b::h2_ready_reply_at_exact_deadline_is_still_unconfirmed"
diff -qr src "$baseline/src"
cmp Cargo.toml "$baseline/Cargo.toml"
cmp Cargo.lock "$baseline/Cargo.lock"
nice -n 10 cargo test --locked --offline admin -- --nocapture >"$audit_dir/restored.log" 2>&1
rg -q 'Compiling quota-throttle' "$audit_dir/restored.log"
rg -q '^test result: ok\. 44 passed; 0 failed; 2 ignored;' "$audit_dir/restored.log"
printf '16 compiled mutations caught; restored exact original sources and genuinely recompiled; 44 passed/0 failed/2 ignored.\n'
