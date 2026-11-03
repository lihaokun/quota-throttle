#!/usr/bin/env bash
set -euo pipefail
# 既有流程与真实 wire 输出保持检查；不放宽历史阶段的基线。
bash scripts/contract_audit/sgm2_worker.sh
git diff --exit-code 6747ef3 -- Cargo.toml Cargo.lock src \
  ':(exclude)src/newapi/transport.rs' \
  ':(exclude)src/newapi/transport/assurance_tests.rs' \
  ':(exclude)src/newapi/managed_channels/tests.rs' \
  ':(exclude)src/reconcile/worker/tests.rs'
rustfmt --edition 2021 --check src/newapi/transport.rs src/newapi/managed_channels/tests.rs src/reconcile/worker/tests.rs
cargo test --locked --offline r10_ -- --test-threads=1
cargo test --locked --offline property_c7_no_secret_in_snapshots_or_debug -- --test-threads=1
cargo test --locked --offline journal_effects_and_debug_exclude_private_material -- --test-threads=1
cargo test --locked --offline operation_views_and_direct_command_debug_do_not_leak_material -- --test-threads=1
cargo test --locked --offline r8_ -- --test-threads=1
printf 'SGM-2 R8/R10 checks passed; backend binding is a separate uncompleted gate\n'
