#!/usr/bin/env bash
# 第一批机械门禁；不声称 P8–P16/OperationView 已实现。
set -euo pipefail
audit_base=f9f02aa
rustfmt --edition 2021 --check src/newapi.rs
git diff --check "$audit_base"
git diff --exit-code "$audit_base" -- src/orchestrator.rs src/quota.rs src/boot.rs src/status.rs src/model_catalog.rs src/config.rs src/catalog/store.rs src/catalog/schema_v1.sql src/catalog/tests.rs

main_changes="$(git diff --unified=0 "$audit_base" -- src/main.rs | sed '/^+++ /d; /^--- /d' | sed -n '/^[+-]/p')"
test "$main_changes" = $'-mod newapi;\n+pub mod newapi;'
secret_changes="$(git diff --unified=0 "$audit_base" -- src/catalog/types.rs | sed '/^+++ /d; /^--- /d' | sed -n '/^[+-]/p')"
test "$secret_changes" = $'-    pub(super) fn expose(&self) -> &str {\n+    pub(crate) fn expose(&self) -> &str {'

sed -n '/pub fn new(/,/^    }/p' src/newapi/transport.rs | rg '// Step P1:' >/dev/null
for step in 2 3; do
  sed -n '/pub async fn send(/,/^    }/p' src/newapi/transport.rs | rg "// Step P${step}:" >/dev/null
done
for step in 4 5; do
  sed -n '/pub async fn scan_channels(/,/^    }/p' src/newapi/managed_channels.rs | rg "// Step P${step}:" >/dev/null
done
sed -n '/pub async fn create_disabled_channel(/,/^    }/p' src/newapi/managed_channels.rs | rg '// Step P6:' >/dev/null
sed -n '/pub async fn write_status(/,/^    }/p' src/newapi/managed_channels.rs | rg '// Step P7:' >/dev/null
test "$(rg -l 'pub enum RequestClass' src | wc -l)" -eq 1
test "$(rg -l 'wire_enum!\(SourceKind' src | wc -l)" -eq 1
if rg 'enum Auth|async fn login|\.client\.' src/newapi.rs; then
  printf 'Legacy client bypasses shared transport\n' >&2
  exit 1
fi
if rg 'Policy::(limited|custom)|CopyChannel|Method::DELETE' src/newapi/transport.rs src/newapi/managed_channels.rs; then
  printf 'Unexpected redirect or destructive management API\n' >&2
  exit 1
fi

# 旧目录真实输出的 schema 仍必须通过；不修改历史 sgm-1 脚本的固定基线。
test_output="$(cargo test --locked --offline catalog::store::tests::catalog_schema_output -- --exact --nocapture)"
snapshot="$(printf '%s\n' "$test_output" | sed -n 's/^CATALOG_SCHEMA_JSON=//p')"
test -n "$snapshot"
check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/catalog-snapshot.schema.json \
  --force-filetype json <(printf '%s\n' "$snapshot")
printf 'sgm-2 batch 1 mechanical checks passed (P1–P7 only)\n'
