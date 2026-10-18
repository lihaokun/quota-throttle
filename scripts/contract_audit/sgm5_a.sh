#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=eb0b727
git diff --check "$base"
while IFS= read -r path; do
  case "$path" in
    src/newapi.rs|src/newapi/transport.rs|src/newapi/tokens.rs|src/newapi/tokens/*|docs/design/source-groups-token-management/*|docs/research/source-groups-token-management-research.md|docs/audits/sgm-5-token-management/*|docs/fixes/newapi-token-fix-auth-principal-drift.md|scripts/contract_audit/sgm5_a.sh|scripts/tests/performance-gates.sh|scripts/tests/performance-gates-regression.sh|scripts/probes/newapi_token_probe.py|proptest-regressions/newapi/tokens/*) ;;
    *) printf 'Unexpected SGM5 A path: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <({ git diff --name-only "$base"; git ls-files --others --exclude-standard; } | LC_ALL=C sort -u)
git diff --exit-code "$base" -- Cargo.toml Cargo.lock src/catalog src/reconcile src/source_policy src/ledger src/main.rs src/boot.rs src/config.rs src/status.rs src/orchestrator.rs src/newapi/managed_channels.rs src/newapi/managed_channels src/newapi/logs.rs src/newapi/logs src/newapi/preflight.rs src/newapi/preflight src/newapi/transport
# 仅剥离本批factory/lookup/固定owner插入后，旧传输及旧client内容必须逐字保留。
diff -B -u <(git show "$base:src/newapi.rs") <(awk '
  /^pub\(crate\) mod tokens;/ {next}
  /^    #\[allow\(dead_code\)\]/ {next}
  /^    pub\(crate\) fn managed_tokens/ {skip=1;next}
  skip && /^    }$/ {skip=0;next}
  skip {next}
  {print}
' src/newapi.rs)
diff -B -u <(git show "$base:src/newapi/transport.rs") <(awk '
  /^    token_lookup: bool,$/ || /^            token_lookup: false,$/ {next}
  /^    expected_user_id: Option<i64>,$/ || /^            expected_user_id: None,$/ {next}
  /^    \/\/\/ 固定版本token单条GET/ || /^    pub\(super\) fn token_missing/ || /^    \/\/\/ 固定业务账号/ {body=1;next}
  body && /^    }$/ {body=0;next}
  body {next}
  /^        \/\/ 固定owner同时约束/ {owner=1;next}
  owner && /^        }$/ {owner=0;next}
  owner {next}
  /^                    if request.token_lookup && response.token_missing\(\)/ {lines=2;next}
  lines>0 {lines--;next}
  {print}
' src/newapi/transport.rs)
for step in T5A.1 T5A.2 T5A.3 T5A.4; do test "$(rg -F "// Step $step:" src/newapi/tokens.rs | wc -l)" -eq 1; done
rg -q '^pub const NATIVE_TOKEN_SCHEMA_VERSION: i64 = 1;' src/newapi/tokens.rs
rustfmt --edition 2021 --config skip_children=true --check src/newapi/transport.rs src/newapi/tokens.rs src/newapi/tokens/tests.rs
python3 -B scripts/probes/newapi_token_probe.py --help >/dev/null
bash scripts/tests/performance-gates-regression.sh
prefix=newapi::tokens::tests
expected="$(printf '%s\n' \
  "$prefix::t5a_view_schema_and_private_material" \
  "$prefix::t5a_bad_fields_are_unknown_not_defaults_and_negative_quota_is_valid" \
  "$prefix::t5a_bad_pages_are_not_empty_success" \
  "$prefix::t5a_invalid_inputs_send_nothing" \
  "$prefix::t5a_signer_readiness_never_falls_back_to_admin" \
  "$prefix::t5a_lookup_recovers_expired_creation_and_never_picks_ambiguous_or_casefolded_name" \
  "$prefix::t5a_missing_is_only_the_explicit_token_lookup_case" \
  "$prefix::t5a_legacy_inheritance_status_and_single_delete_preserve_other_fields" \
  "$prefix::t5a_reveal_is_explicit_prefixed_once_and_errors_are_redacted" \
  "$prefix::t5a_write_gate_is_rechecked_after_preflight" \
  "$prefix::t5a_401_replay_rechecks_gate_and_keeps_owner_session" \
  "$prefix::t5a_two_owner_sessions_share_budget_not_cookie" \
  "$prefix::t5a_bound_owner_survives_reauthentication" \
  "$prefix::t5a_bound_owner_bearer_header_overrides_static_value" \
  "$prefix::t5a_uncertain_writes_are_not_automatically_retried" \
  "$prefix::t5a_expiry_elapsed_before_final_send_prevents_create" \
  "$prefix::t5a_non_200_success_is_not_a_token_ack_or_safe_preflight" \
  "$prefix::t5a_available_groups_are_owner_checked_and_auto_is_not_selectable" \
  "$prefix::t5a_cancel_after_send_does_not_retry_or_hold_budget" \
  "$prefix::t5a_status_response_drift_is_uncertain_not_success" \
  "$prefix::t5a_pb1_projection_matches_independent_mask_and_legacy_group" \
  "$prefix::t5a_pb2_create_payload_is_single_group_whitelist" \
  "$prefix::t5a_pb3_owner_and_id_drift_never_produces_status_write" \
  "$prefix::performance_gate_token_maximum_page" | LC_ALL=C sort)"
expected_ignored="$prefix::performance_gate_token_maximum_page"
manifest_ok() { [[ -n "$1" && "$1" == "$expected" && "$2" == "$expected_ignored" && "$1" != "$2" ]]; }
if manifest_ok '' '' || manifest_ok "$expected" "$expected" || manifest_ok "${expected#*$'\n'}" "$expected_ignored"; then exit 1; fi
listing="$(cargo test --locked --offline -j 2 -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^newapi::tokens::tests::' | LC_ALL=C sort || true)"
listing="$(cargo test --locked --offline -j 2 -- --ignored --list)"
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^newapi::tokens::tests::' | LC_ALL=C sort || true)"
manifest_ok "$actual" "$ignored" || { printf 'Token A manifest mismatch\n' >&2; exit 1; }
output="$(cargo test --locked --offline -j 2 t5a_view_schema_and_private_material -- --nocapture)"
view="$(printf '%s\n' "$output" | sed -n 's/^TOKEN_SCHEMA_JSON=//p')"
enums="$(printf '%s\n' "$output" | sed -n 's/^TOKEN_ENUM_JSON=//p')"
schema=docs/design/source-groups-token-management/schemas/native-token.schema.json
check-jsonschema --schemafile "$schema" --force-filetype json <(printf '%s\n' "$view")
for field in owner_scope status; do
  test "$(printf '%s\n' "$enums" | jq -c --arg f "$field" '.[$f]|sort')" == "$(jq -c --arg f "$field" '.properties[$f].enum|sort' "$schema")"
done
for expression in '.key="plaintext"' '.schema_version=2' '.masked_key="unmasked"' '.owner_scope="admin"' '.token_id="0"'; do
  bad="$(printf '%s\n' "$view" | jq -c "$expression")"
  if check-jsonschema --schemafile "$schema" --force-filetype json <(printf '%s\n' "$bad") >/dev/null 2>&1; then exit 1; fi
done
printf 'SGM5 A scope, frozen old transport/client, schema/enum/Step and exact 24-test manifest passed; regression, mutations, native probe and performance are separate.\n'
