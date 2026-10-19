#!/usr/bin/env python3
"""SGM7 mechanical contracts; behavioral evidence remains in tests and real C probe."""
import argparse
import ast
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]


def run(*args):
    return subprocess.check_output(args, cwd=ROOT, text=True, stderr=subprocess.STDOUT)


def check(freeze):
    read = lambda name: (ROOT / name).read_text()
    runtime = read("src/runtime.rs") + read("src/runtime/config.rs")
    for step in range(1, 6):
        assert f"Step R{step}:" in runtime, f"missing R{step}"
    assert "Orchestrator" not in runtime
    assert "SUPPORTED_BACKEND_VERSION" in read("src/runtime/config.rs")
    assert "use crate::catalog::control::ControlErrorCode;" in runtime
    assert "ManagementBudget::new()" in runtime and runtime.count("ManagementBudget::new()") == 1
    assert "issuer" in runtime and "cfg.admin_token.clear()" in runtime
    assert "cfg.extra_headers.clear()" in runtime
    assert "abort_all" not in runtime and "detach_all" not in runtime
    assert "args.first()" in read("src/main.rs") or "parse_command" in read("src/main.rs")
    probe = ast.parse(read("scripts/probes/groups_runtime_probe.py"))
    checks = next(ast.literal_eval(node.value) for node in probe.body
                  if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == "CHECKS" for t in node.targets))
    assert len(checks) == len(set(checks)) == 15
    marks = [node.args[0].value for node in ast.walk(probe)
             if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
             and node.func.id == "mark" and node.args and isinstance(node.args[0], ast.Constant)]
    assert sorted(marks) == sorted(checks)
    browser = read("scripts/tests/groups-browser.mjs")
    assert ".route(" not in browser and "apiMocks: false" in browser
    assert "assert.equal(completed.length, 4)" in browser
    tests = run("cargo", "test", "--locked", "--offline", "--", "--list")
    names = sorted(line.removesuffix(": test") for line in tests.splitlines() if line.endswith(": test")
                   and (line.startswith("runtime::tests::") or "::r7_" in line
                        or line.startswith("newapi::preflight::runtime::tests::")
                        or line.startswith("cli_tests::")))
    expected = read("scripts/contract_audit/sgm7_tests.txt").splitlines()
    assert names == expected, (names, expected)
    ignored = run("cargo", "test", "--locked", "--offline", "--", "--ignored", "--list")
    assert not any(name + ": test" in ignored for name in names)
    if freeze:
        run("git", "diff", "--check")
        run("git", "diff", "--exit-code", "ab59a88", "--", "Cargo.toml", "Cargo.lock", "config.example.toml",
            "src/config.rs", "src/orchestrator.rs", "src/status.rs", "src/catalog", "src/catalog.rs",
            "src/source_policy", "src/source_policy.rs", "src/ledger", "src/ledger.rs",
            "docs/design/source-groups-token-management/schemas")
    print(f"SGM7: R1–R5; shared types/budget; {len(names)} exact runnable Rust tests; manifests only: 15 C checks + 4 unmocked browsers;" +
          (" legacy policies/schema frozen." if freeze else " frozen checks require local git --freeze."))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--freeze", action="store_true")
    check(parser.parse_args().freeze)
