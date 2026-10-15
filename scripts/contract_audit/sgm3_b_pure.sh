#!/usr/bin/env bash
set -euo pipefail

base=7f4fe7b
git diff --check "$base"
git diff --exit-code "$base" -- src Cargo.toml Cargo.lock \
  ':(exclude)src/catalog/types.rs' ':(exclude)src/source_policy/mod.rs' \
  ':(exclude)src/source_policy/volcano.rs' ':(exclude)src/source_policy/volcano/'
while IFS= read -r path; do
  case "$path" in
    src/source_policy/volcano.rs|src/source_policy/volcano/*|scripts/contract_audit/sgm3_b_pure.sh|docs/audits/sgm-3-source-policies/*|proptest-regressions/source_policy/volcano/*) ;;
    *) printf 'Unexpected untracked file: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <(git ls-files --others --exclude-standard)
for step in 1 2 3 4 5 6 7; do
  test "$(rg -c "// Step B$step:" src/source_policy/volcano.rs)" -eq 1
done
if rg 'reqwest|rusqlite|SystemTime|std::fs|\.await|NewApiClient|CatalogStore' src/source_policy/volcano.rs; then
  printf 'Volcano policy must remain pure\n' >&2; exit 1
fi
if rg 'enum (SourceKind|MembershipLifecycle|Regime)' src/source_policy/volcano.rs; then
  printf 'Shared enums must not be redefined\n' >&2; exit 1
fi
rustfmt --edition 2021 --check src/source_policy/volcano.rs
cargo test --locked --offline -j 2 source_policy::volcano -- --test-threads=4
printf 'SGM-3 B pure checks passed; database B8-B10 and full B acceptance remain separate\n'
