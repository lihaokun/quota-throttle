#!/usr/bin/env bash
# 编译且命中语义断言才计数；仅改独立副本，绝不修改运行/全量测试的源或target。
set -euo pipefail
baseline=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
audit_dir=$(mktemp -d /tmp/quota-throttle-sgm7-mutations.XXXXXX)
printf 'Mutation workspace: %s\n' "$audit_dir"
cp -a "$baseline/src" "$baseline/Cargo.toml" "$baseline/Cargo.lock" "$baseline/config.example.toml" "$audit_dir/"
cp -a --reflink=auto "$baseline/target" "$audit_dir/target"
cd "$audit_dir"
export CARGO_TARGET_DIR="$audit_dir/target" CARGO_NET_OFFLINE=true CARGO_BUILD_JOBS=8
files=(src/runtime/config.rs src/runtime.rs src/newapi/preflight/runtime.rs src/reconcile/worker/policy.rs)
restore() { local file; for file in "${files[@]}"; do cp "$baseline/$file" "$file"; touch "$file"; done; }
trap restore EXIT
replace() {
  BEFORE="$2" AFTER="$3" perl -0777 -i -pe '
    BEGIN { $a=$ENV{BEFORE}; $b=$ENV{AFTER}; }
    $n=()=/\Q$a\E/g; die "expected exactly one mutation match, got $n\n" if $n != 1;
    s/\Q$a\E/$b/;
  ' "$1"
}
run_case() {
  local label=$1 test_name=$2 assertion=$3 status
  set +e
  timeout 480 cargo test --locked --offline "$test_name" -- --exact --nocapture >"$audit_dir/$label.log" 2>&1
  status=$?
  set -e
  [[ "$status" == 101 ]]
  grep -q 'Compiling quota-throttle' "$audit_dir/$label.log"
  grep -q '^test result: FAILED\. 0 passed; 1 failed; 0 ignored;' "$audit_dir/$label.log"
  grep -Fq "$assertion" "$audit_dir/$label.log"
  ! grep -Eq '^error\[|could not compile|test result: ok|PermissionDenied|No space left on device' "$audit_dir/$label.log"
  printf '%s: compiled; exact semantic test failed\n' "$label"
  restore
}
restore
replace src/runtime/config.rs '|| !policy.keys.is_empty()' '|| (false && !policy.keys.is_empty())'
run_case m01_legacy_sources_admitted runtime::tests::r1_bad_configuration_is_rejected_without_secret_echo 'called `Option::unwrap()` on a `None` value'
replace src/newapi/preflight/runtime.rs '("channel_affinity_setting.enabled", "false"),' '("channel_affinity_setting.enabled", "true"),'
run_case m02_native_affinity_allowed newapi::preflight::runtime::tests::r2_environment_and_route_settings_fail_closed 'assertion failed: settings_ready(&valid)'
replace src/runtime.rs $'        | Err(AnalyticsError::Ledger(LedgerError::NotInitialized))\n' ''
run_case m03_fresh_ledger_is_fatal runtime::tests::r5_fresh_ledger_waits_for_collection_instead_of_terminating_service 'left: Err(Storage)'
# 以下替换锚定火山入口的唯一相邻文本，不能同时变更GLM提交。
replace src/reconcile/worker/policy.rs $'        let _automatic = self\n            .automatic_updates\n            .try_read()\n            .map_err(|_| error(ControlErrorCode::QueueFull))?;\n        self.publish(|c| {' $'        self.publish(|c| {'
run_case m04_automatic_publication_during_registration reconcile::worker::tests::token_service_tests::runtime_cooperation::r7_actual_group_check_excludes_background_publication_without_changing_epoch 'called `Result::unwrap_err()` on an `Ok` value'
diff -qr src "$baseline/src"
cargo test --locked --offline runtime -- --nocapture >"$audit_dir/restored.log" 2>&1
grep -q 'Compiling quota-throttle' "$audit_dir/restored.log"
grep -Eq '^test result: ok\. [1-9][0-9]* passed; 0 failed;' "$audit_dir/restored.log"
printf '4 compiled mutations caught; original source restored, recompiled and rechecked.\n'
