#!/usr/bin/env bash
set -euo pipefail

base=0b52e50
git diff --check "$base"
git diff --exit-code "$base" -- src Cargo.toml Cargo.lock \
  ':(exclude)src/catalog/store.rs' ':(exclude)src/catalog/types.rs' \
  ':(exclude)src/catalog/mod.rs' ':(exclude)src/catalog/policy.rs' \
  ':(exclude)src/catalog/policy_tests.rs' ':(exclude)src/catalog/schema_v4.sql' \
  ':(exclude)src/catalog/tests.rs' ':(exclude)src/catalog/control_tests.rs' \
  ':(exclude)src/catalog/execution_tests.rs'
git show "$base:src/catalog/types.rs" | \
  sed 's/DATABASE_SCHEMA_VERSION: i64 = 3/DATABASE_SCHEMA_VERSION: i64 = 4/' | \
  diff - src/catalog/types.rs
while IFS= read -r path; do
  case "$path" in
    src/catalog/policy.rs|src/catalog/policy_tests.rs|src/catalog/schema_v4.sql|scripts/contract_audit/sgm3_b_storage.sh|docs/audits/sgm-3-source-policies/*|proptest-regressions/catalog/*) ;;
    *) printf 'Unexpected untracked file: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <(git ls-files --others --exclude-standard)
test "$(rg -c '// Step B8:' src/catalog/store.rs)" -eq 1
for step in 9 10; do
  test "$(rg -c "// Step B$step:" src/catalog/policy.rs)" -eq 1
done
if rg 'reqwest|\.await|SystemTime|std::fs|NewApiClient|enum (SourceKind|Regime|MembershipLifecycle)' src/catalog/policy.rs; then
  printf 'Policy persistence must reuse shared types and have no network/clock side effects\n' >&2; exit 1
fi
rustfmt --edition 2021 --check src/catalog/policy.rs src/catalog/policy_tests.rs src/catalog/store.rs
cargo test --locked --offline -j 2 catalog::store::tests::policy -- --test-threads=4
printf 'SGM-3 B storage checks passed; complex historical migration B5 remains separate\n'
