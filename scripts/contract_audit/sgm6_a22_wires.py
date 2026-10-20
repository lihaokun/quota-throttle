#!/usr/bin/env python3
"""Check captured implementation wires against each route's frozen definition, offline."""
import copy
import json
import re
import sys
from collections import Counter
from pathlib import Path

from jsonschema import Draft202012Validator


def main():
    root = Path(__file__).resolve().parents[2]
    directory = Path(sys.argv[1]).resolve(strict=True)
    assert directory.is_relative_to("/tmp"), "isolated fixture directory required"
    schemas = root / "docs/design/source-groups-token-management/schemas"
    originals = {
        family: json.loads((schemas / f"panel-admin-{family}s.schema.json").read_text())
        for family in ("request", "response")
    }
    for schema in originals.values():
        Draft202012Validator.check_schema(schema)
    for name in ("catalog-snapshot", "control-operation", "token-identity", "native-token", "group-check", "group-references"):
        assert originals["response"]["$defs"]["original_" + name.replace("-", "_")] == json.loads((schemas / f"{name}.schema.json").read_text())
    counts = Counter()
    validators = {}
    first = {}
    for file in sorted(directory.glob("*.json")):
        match = re.fullmatch(r"(request|response)_([A-Za-z0-9_]+)-[0-9]+-[0-9]+\.json", file.name)
        assert match, f"unexpected fixture: {file.name}"
        family, definition = match.groups()
        key = (family, definition)
        if key not in validators:
            schema = copy.deepcopy(originals[family])
            schema.pop("anyOf")
            schema["$ref"] = f"#/$defs/{definition}"
            validators[key] = Draft202012Validator(schema)
        value = json.loads(file.read_text())
        errors = list(validators[key].iter_errors(value))
        assert not errors, f"{file.name}: {errors[0].json_path}: {errors[0].message}" if errors else ""
        counts[key] += 1
        first.setdefault(key, value)
    expected_requests = {
        "CreateSource", "CreateGroup", "PlanMembership", "OperationRevision", "SetModels",
        "SetSourcePaused", "ClearVolcanoError", "UpdateVolcanoPolicy", "CheckGroup", "SetGroupPin",
        "VersionOnly", "CreateToken", "SetTokenEnabled", "RevealToken", "RegisterToken", "UsageQuery",
        "IdentityQuery", "NativeTokenQuery",
    }
    expected_responses = {
        "SourceCreated", "GroupCreated", "MemberPlanned", "SourceModels", "IdentityPage", "NativePage",
        "TokenReveal", "UsageSnapshot", "AdminError", "original_control_operation", "original_token_identity",
        "original_group_check", "original_group_references",
    }
    assert set(counts) == ({("request", k) for k in expected_requests} | {("response", k) for k in expected_responses}), counts
    negatives = 0
    # Every implemented root is a strict object with a mandatory schema version (query DTOs excepted).
    for key, value in first.items():
        bad = copy.deepcopy(value)
        bad["private_extra"] = "fake-audit-only"
        assert list(validators[key].iter_errors(bad)), key
        negatives += 1
        if "schema_version" in value:
            bad = copy.deepcopy(value)
            bad.pop("schema_version")
            assert list(validators[key].iter_errors(bad)), key
            negatives += 1
    error_validator = validators[("response", "AdminError")]
    codes = set(originals["response"]["$defs"]["AdminError"]["properties"]["error"]["enum"])
    emitted = set()
    for file in directory.glob("response_AdminError-*.json"):
        value = json.loads(file.read_text())
        emitted.add(value["error"])
        bad = copy.deepcopy(value)
        bad["existing_source_id"] = None if value["error"] == "duplicate_source" else "1"
        assert list(error_validator.iter_errors(bad))
    assert len(codes) == 28 and emitted == codes, emitted
    print(f"A2.2 actual wires: {sum(counts.values())} positives across 18 request and 13 response definitions; {negatives} root negatives; all 28 error contexts; 6 old schemas identical.")


if __name__ == "__main__":
    main()
