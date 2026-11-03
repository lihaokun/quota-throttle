#!/usr/bin/env bash
set -euo pipefail
# 不放宽历史脚本；沿用 wire/步骤检查，另固定本轮只有测试增量。
bash scripts/contract_audit/sgm2_worker.sh
git diff --exit-code 4b4a79e -- Cargo.toml Cargo.lock src \
  ':(exclude)src/reconcile/worker/tests.rs' \
  ':(exclude)src/reconcile/worker/scenarios.rs'
rustfmt --edition 2021 --check src/reconcile/worker/tests.rs
# 全量包含 R8/R10 及目录/旧策略回归，不能只跑新增 scenario 子集。
cargo test --locked --offline --quiet
printf 'SGM-2 scenario checkpoint checks passed; real backend preflight remains pending\n'
