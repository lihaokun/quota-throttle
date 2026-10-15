#!/usr/bin/env bash
set -euo pipefail

base=e1a9279
git diff --check "$base"
git diff --exit-code "$base" -- Cargo.toml Cargo.lock src \
  ':(exclude)src/main.rs' ':(exclude)src/orchestrator.rs' ':(exclude)src/source_policy/'
diff -u <(git show "$base:src/main.rs" | sed '/^pub mod reconcile;$/a pub mod source_policy;') src/main.rs

# 新文件也受白名单约束，不能只检查已跟踪 diff。
while IFS= read -r path; do
  case "$path" in
    src/source_policy/*|docs/audits/sgm-3-source-policies/*|docs/design/source-groups-token-management/subplans/sgm-3-source-policies.md|scripts/contract_audit/sgm3_batch_a.sh|proptest-regressions/source_policy/*) ;;
    *) printf 'Unexpected untracked file: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <(git ls-files --others --exclude-standard)

for function in max_watch_pct imminent eligible_set decide; do
  diff -u \
    <(git show "$base:src/orchestrator.rs" | sed -n "/^fn $function(/,/^}/p") \
    <(sed 's/^pub(crate) fn /fn /' src/orchestrator.rs | sed -n "/^fn $function(/,/^}/p")
done
# 冻结参考不能跟随生产算法改变。
for function in imminent eligible_set decide; do
  diff -u \
    <(git show "$base:src/orchestrator.rs" | sed -n "/^fn $function(/,/^}/p" | rustfmt --edition 2021) \
    <(sed 's/^pub(super) fn /fn /' src/source_policy/reference.rs | sed -n "/^fn $function(/,/^}/p")
done

for step in 1 2 3 4 5 6 7; do
  test "$(rg -l "// Step P$step:" src/source_policy/probe.rs src/source_policy/glm.rs | wc -l)" -eq 1
done
if rg 'enum (SourceKind|MembershipLifecycle|Regime)' src/source_policy; then
  printf 'Shared enums must not be redefined\n' >&2; exit 1
fi
if rg 'reqwest|NewApiClient|CatalogStore|\.submit\(|\.publish' src/source_policy/glm.rs; then
  printf 'GLM adapter must remain pure\n' >&2; exit 1
fi
rustfmt --edition 2021 --check src/source_policy/mod.rs
cargo test --locked --offline source_policy -- --test-threads=4
printf 'SGM-3 batch A mechanical checks passed; full regression and independent audit are separate gates\n'
