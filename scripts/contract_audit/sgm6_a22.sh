#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.."
base=bcb5700
git diff --check "$base"
while IFS= read -r path; do
  case "$path" in
    src/panel_http.rs|src/panel_http/*|src/catalog/admin.rs|src/catalog/mod.rs|src/catalog/control.rs|src/catalog/types.rs|src/catalog/ledger/types.rs|src/reconcile/worker.rs|src/reconcile/worker/admin.rs|src/reconcile/worker/analytics/tests.rs|src/reconcile/worker/token_service_tests.rs|scripts/contract_audit/sgm6_a22*|scripts/tests/performance-gates.sh|scripts/tests/performance-gates-regression.sh|docs/design/source-groups-token-management/*|docs/audits/sgm-6-three-page-panel/*|docs/fixes/panel-admin-fix-strict-json-shapes.md|proptest-regressions/panel_http/*) ;;
    *) printf 'Unexpected A2.2 path: %s\n' "$path" >&2; exit 1 ;;
  esac
done < <({ git diff --name-only "$base"; git ls-files --others --exclude-standard; } | LC_ALL=C sort -u)
git diff --exit-code "$base" -- Cargo.toml Cargo.lock config.example.toml src \
  ':(exclude)src/panel_http.rs' ':(exclude)src/panel_http' \
  ':(exclude)src/catalog/admin.rs' ':(exclude)src/catalog/mod.rs' ':(exclude)src/catalog/control.rs' ':(exclude)src/catalog/types.rs' ':(exclude)src/catalog/ledger/types.rs' \
  ':(exclude)src/reconcile/worker.rs' ':(exclude)src/reconcile/worker/admin.rs' ':(exclude)src/reconcile/worker/analytics/tests.rs' ':(exclude)src/reconcile/worker/token_service_tests.rs'
git diff --exit-code "$base" -- docs/design/source-groups-token-management/schemas src/panel_http/request.rs src/panel_http/tests
diff -u <(git show "$base:src/panel_http/tests.rs") <(perl -0777 -pe '
  $n=s/\nimpl PanelServer \{\n    \/\/ H1用自定义publisher.*?\n\}\n(?=\npub\(crate\) fn empty)//s; die "missing H1 worker fixture adapter" unless $n==1;
  $n=s/^    let fixture = admin::tests::support::fixture\(false\);\n    let \(business, _supervisor\) = admin::Supervisor::new\(fixture.worker.clone\(\)\);\n//m; die "missing H1 connection fixture" unless $n==1;
  $n=s/^        business,\n//m; die "missing H1 connection argument" unless $n==1;
' src/panel_http/tests.rs)
diff -u <(git show "$base:src/catalog/mod.rs") <(sed '/^pub(crate) mod admin;$/d' src/catalog/mod.rs)
diff -u <(git show "$base:src/reconcile/worker.rs") <(sed '/^mod admin;$/d' src/reconcile/worker.rs)
diff -u <(git show "$base:src/catalog/control.rs") <(sed 's/^pub(crate) fn canonical_models/fn canonical_models/' src/catalog/control.rs)
diff -u <(git show "$base:src/catalog/types.rs") <(sed -e 's/^pub(crate) fn valid_name/pub(super) fn valid_name/' -e 's/^pub(crate) fn validate_url/fn validate_url/' src/catalog/types.rs)
diff -u <(git show "$base:src/catalog/ledger/types.rs") <(sed 's/pub(crate) fn as_str/pub(super) fn as_str/' src/catalog/ledger/types.rs)
diff -u <(git show "$base:src/reconcile/worker/analytics/tests.rs") <(sed -e '/^#\[path = "..\/..\/..\/panel_http\/admin\/tests\/query_tail.rs"\]$/d' -e '/^mod panel_admin;$/d' src/reconcile/worker/analytics/tests.rs)
diff -u <(git show "$base:src/reconcile/worker/token_service_tests.rs") <(sed -e '/^\/\/ 复用原生token假后端，只从新增HTTP入口驱动，避免复制身份\/渠道夹具。$/d' -e '/^#\[path = "..\/..\/panel_http\/admin\/tests\/tokens.rs"\]$/d' -e '/^mod panel_admin;$/d' src/reconcile/worker/token_service_tests.rs)
for file in scripts/tests/performance-gates.sh scripts/tests/performance-gates-regression.sh; do
  test "$(rg -c '::performance_gate_admin_' "$file")" -eq 2
  diff -u <(git show "$base:$file") <(sed -E '/^ +panel_http::admin::(tests|output::tests)::performance_gate_admin_(request_decode|usage_encoding)( \\)?$/d' "$file")
done
for step in H1.1 H1.2 H1.3 H1.4 H1.5 H1.6 H1.7 H2.1 H2.2 H2.3 H2.4 H2.5 H2.6 H2.7; do
  test "$(rg -F "// Step $step:" src/panel_http.rs src/panel_http | wc -l)" -eq 1
done
test "$(rg -F -c '.panel_snapshot()' src/panel_http.rs)" -eq 1
test "$(rg -F -c 'self.jobs.spawn_blocking' src/panel_http/admin/bridge.rs)" -eq 1
if rg -n 'NewApiClient|CatalogStore|read_source_material|source_secrets|send_management' src/panel_http/admin/{bridge.rs,dispatch.rs,output.rs,input.rs,route.rs} src/panel_http/admin/output/usage.rs; then exit 1; fi
if rg -n 'source_secrets|read_source_material' src/catalog/admin.rs; then exit 1; fi
mapfile -t format_files < <(rg --files src/panel_http | rg '\.rs$')
rustfmt --edition 2021 --config skip_children=true --check src/panel_http.rs src/catalog/admin.rs src/reconcile/worker/admin.rs "${format_files[@]}"
bash scripts/tests/performance-gates-regression.sh
expected="$(cat scripts/contract_audit/sgm6_a22_tests.txt)"
expected_ignored="$(printf '%s\n' "$expected" | rg '::performance_gate_')"
test "$(printf '%s\n' "$expected" | wc -l)" -eq 80
test "$(printf '%s\n' "$expected_ignored" | wc -l)" -eq 4
test "$(printf '%s\n' "$expected" | rg -c '::(h2_)?pb[1-6]_')" -eq 6
manifest_ok() { [[ -n "$1" && "$1" == "$expected" && "$2" == "$expected_ignored" && "$1" != "$2" ]]; }
if manifest_ok '' '' || manifest_ok "$expected" "$expected" || manifest_ok "${expected#*$'\n'}" "$expected_ignored"; then exit 1; fi
listing="$(cargo test --locked --offline -j 2 -- --list)"
actual="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^panel_http::|::panel_admin::' | LC_ALL=C sort)"
listing="$(cargo test --locked --offline -j 2 -- --ignored --list)"
ignored="$(printf '%s\n' "$listing" | sed -n 's/: test$//p' | rg '^panel_http::|::panel_admin::' | LC_ALL=C sort)"
manifest_ok "$actual" "$ignored" || { printf 'A2.2 exact manifest mismatch\n' >&2; diff -u <(printf '%s\n' "$expected") <(printf '%s\n' "$actual"); exit 1; }
audit_dir=$(mktemp -d /tmp/quota-throttle-sgm6-a22-wires.XXXXXX)
printf 'Implementation wire fixtures: %s\n' "$audit_dir"
export SGM6_A22_WIRE_DIR="$audit_dir"
cargo test --locked --offline -j 2 admin -- --nocapture
python3 scripts/contract_audit/sgm6_a22_wires.py "$audit_dir"
printf 'A2.2 core/schema freeze, 14 Steps, exact 80/4 manifest, six PBT groups and implementation wire schemas passed; full profiles/mutations/performance remain separate.\n'
