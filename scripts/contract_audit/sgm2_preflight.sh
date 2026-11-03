#!/usr/bin/env bash
set -euo pipefail
git diff --check 713803f
git diff --exit-code 713803f -- Cargo.toml Cargo.lock src \
  ':(exclude)src/boot.rs' ':(exclude)src/newapi.rs' \
  ':(exclude)src/newapi/preflight.rs' ':(exclude)src/newapi/preflight/tests.rs' \
  ':(exclude)src/newapi/transport.rs' ':(exclude)src/newapi/managed_channels/tests.rs' \
  ':(exclude)src/reconcile/worker.rs' ':(exclude)src/reconcile/worker/tests.rs'
rustfmt --edition 2021 --check src/newapi/preflight.rs src/newapi/transport.rs src/reconcile/worker.rs src/newapi/managed_channels/tests.rs
if rg 'Method::(PUT|POST)|/api/setup|\.stop\(|ensure_running|one-api\.db|SQLITE_OPEN_READ_WRITE' src/newapi/preflight.rs; then
  printf 'Preflight must not alter the managed service or database\n' >&2
  exit 1
fi
sed -n '/pub async fn check_local_backend(/,/^    }/p' src/newapi/preflight.rs | rg 'timeout\(CHECK_TIMEOUT' >/dev/null
sed -n '/async fn refresh_from(/,/^    }/p' src/reconcile/worker.rs | rg 'c.input_epoch != revoked' >/dev/null
output="$(cargo test --locked --offline confirmation_operation_schema_output -- --nocapture)"
views="$(printf '%s\n' "$output" | sed -n 's/^CONFIRMATION_SCHEMA_JSON=//p')"
test -n "$views"
while IFS= read -r view; do
  check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/control-operation.schema.json --force-filetype json <(printf '%s\n' "$view")
done < <(printf '%s\n' "$views" | jq -c '.[]')
output="$(cargo test --locked --offline catalog_schema_output -- --nocapture)"
snapshot="$(printf '%s\n' "$output" | sed -n 's/^CATALOG_SCHEMA_JSON=//p')"
check-jsonschema --schemafile docs/design/source-groups-token-management/schemas/catalog-snapshot.schema.json --force-filetype json <(printf '%s\n' "$snapshot")
cargo test --locked --offline --quiet -- --test-threads=4
printf 'SGM-2 local preflight checks passed; native integration runs separately and production remains untouched\n'
