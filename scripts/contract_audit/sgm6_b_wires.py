"""UI builder outputs against the existing full schemas; no network or production data."""
import copy
import json
import re
import sys
from pathlib import Path
from urllib.parse import urlsplit
from jsonschema import Draft202012Validator, FormatChecker
from referencing import Registry, Resource

root = Path(__file__).resolve().parents[2]
directory = Path(sys.argv[1]).resolve(strict=True)
assert str(directory).startswith('/tmp/quota-throttle-sgm6-b-')
schemas = root / 'docs/design/source-groups-token-management/schemas'
checker = FormatChecker()
registry = Registry().with_resources(
    (value['$id'], Resource.from_contents(value))
    for value in (json.loads(path.read_text()) for path in schemas.glob('*.schema.json'))
)

@checker.checks('uri', raises=(ValueError,))
def ui_http_uri(value):
    # Narrow explicit UiConfig contract, independent of optional RFC format packages.
    if not isinstance(value, str):
        return True
    u = urlsplit(value)
    return (u.scheme in ('http', 'https') and bool(u.hostname)
            and u.username is None and u.password is None
            and not any(c.isspace() for c in value)
            and not re.search(r'%(?![0-9A-Fa-f]{2})', u.path)
            and not re.search(r"[^A-Za-z0-9\-._~!$&'()*+,;=:@/%]", u.path))

assert checker.checkers.get('uri') is not None
assert not checker.conforms('not a uri', 'uri')
wires = json.loads((directory / 'ui-wires.json').read_text())
covered = set()
balance_negatives = 0
for wire in wires:
    family, definition, value = wire['family'], wire['definition'], wire['value']
    if family in ('request', 'response'):
        schema = json.loads((schemas / f'panel-admin-{family}s.schema.json').read_text())
        schema.pop('anyOf')
        schema['$ref'] = f'#/$defs/{definition}'
    else:
        schema = json.loads((schemas / ('panel-snapshot.schema.json' if family == 'state' else 'panel-ui-config.schema.json')).read_text())
    validator = Draft202012Validator(schema, format_checker=checker, registry=registry)
    validator.validate(value)
    bad = copy.deepcopy(value)
    bad['unexpected_private_field'] = 'synthetic'
    assert list(validator.iter_errors(bad))
    if family == 'state' and value['balance']['unit'] is not None:
        for display_type in ['USD\n', 'A' * 16 + '\n']:
            bad = copy.deepcopy(value)
            bad['balance']['unit']['display_type'] = display_type
            assert list(validator.iter_errors(bad)), repr(display_type)
            balance_negatives += 1
    if definition == 'BalanceRefilled':
        for field, replacement in [('user_id', 2), ('amount', 0), ('amount', 4000000000000001)]:
            bad = copy.deepcopy(value)
            bad[field] = replacement
            assert list(validator.iter_errors(bad)), (definition, field)
            balance_negatives += 1
        bad = copy.deepcopy(value)
        del bad['after']
        assert list(validator.iter_errors(bad))
        balance_negatives += 1
    if family == 'state' and value['balance']['accounts']:
        for field in ['user_id', 'quota', 'observed_at_ms', 'unknown_reason', 'locked_until_ms', 'backoff_until_ms', 'last_refill']:
            bad = copy.deepcopy(value)
            del bad['balance']['accounts'][0][field]
            assert list(validator.iter_errors(bad)), field
            balance_negatives += 1
        bad = copy.deepcopy(value)
        bad['balance']['accounts'][0]['user_id'] = 2
        assert list(validator.iter_errors(bad))
        balance_negatives += 1
        record = value['balance']['accounts'][0]['last_refill']
        if record:
            for field in ['amount', 'error_code']:
                bad = copy.deepcopy(value)
                del bad['balance']['accounts'][0]['last_refill'][field]
                assert list(validator.iter_errors(bad))
                balance_negatives += 1
            bad = copy.deepcopy(value)
            invalid = bad['balance']['accounts'][0]['last_refill']
            invalid['error_code'] = 'not_ready' if record['outcome'] in ('refilled', 'uncertain') else None
            assert list(validator.iter_errors(bad))
            balance_negatives += 1
            if record['outcome'] != 'not_sent':
                bad = copy.deepcopy(value)
                bad['balance']['accounts'][0]['last_refill']['amount'] = None
                assert list(validator.iter_errors(bad))
                balance_negatives += 1
    covered.add((family, definition))
assert len(wires) >= 36 and len(covered) == 25, covered
config = json.loads((schemas / 'panel-ui-config.schema.json').read_text())
validator = Draft202012Validator(config, format_checker=checker)
for url in ['http://example.test/v1', 'http://example.test/%56%31/', 'http://example.test/%zz/proxy', 'http://example.test/a|b', 'not a uri', 'http://u:p@example.test']:
    assert list(validator.iter_errors({'schema_version': 1, 'dry_run': False, 'public_api_base_url': url})), url
assert balance_negatives >= 50, balance_negatives
print(f'Full UI wire schemas: {len(wires)} positives and unknown-field negatives across {len(covered)} families; {balance_negatives} balance negatives; explicit URI checker and six URL negatives.')

# 当前批的Rust捕获验证；冻结的A2.2旧清单保持历史用途。
if len(sys.argv) > 2:
    actual = Path(sys.argv[2]).resolve(strict=True)
    assert actual.is_relative_to('/tmp')
    expected = {
        'request': {'CreateSource', 'CreateGroup', 'PlanMembership', 'OperationRevision', 'SetModels', 'SetSourcePaused', 'ClearVolcanoError', 'UpdateVolcanoPolicy', 'CheckGroup', 'SetGroupPin', 'VersionOnly', 'CreateToken', 'SetTokenEnabled', 'RevealToken', 'RegisterToken', 'UsageQuery', 'IdentityQuery', 'NativeTokenQuery', 'RefillBalance', 'SetBalancePolicy'},
        'response': {'SourceCreated', 'GroupCreated', 'MemberPlanned', 'SourceModels', 'IdentityPage', 'NativePage', 'TokenReveal', 'UsageSnapshot', 'AdminError', 'original_control_operation', 'original_token_identity', 'original_group_check', 'original_group_references', 'BalanceRefilled', 'BalancePolicySaved'},
    }
    seen = {'request': set(), 'response': set()}
    count = 0
    for path in sorted(actual.glob('*.json')):
        match = re.fullmatch(r'(request|response)_([A-Za-z0-9_]+)-([0-9]+)-([0-9]+)\.json', path.name)
        assert match, path.name
        family, definition, pid, pair = match.groups()
        schema = json.loads((schemas / f'panel-admin-{family}s.schema.json').read_text())
        schema.pop('anyOf')
        schema['$ref'] = f'#/$defs/{definition}'
        value = json.loads(path.read_text())
        Draft202012Validator(schema, format_checker=checker, registry=registry).validate(value)
        if family == 'response' and definition in ('BalanceRefilled', 'BalancePolicySaved'):
            context = json.loads((actual / 'balance-context' / path.name).read_text())
            request_family = 'RefillBalance' if definition == 'BalanceRefilled' else 'SetBalancePolicy'
            request_file = f'request_{request_family}-{pid}-{pair}.json'
            assert context == {'request_file': request_file}
            assert (actual / request_file).is_file()
        seen[family].add(definition)
        count += 1
    assert seen == expected, seen
    print(f'Actual Rust schemas: {count} wires across 20 request and 15 response families; balance pair identity verified.')
