"""UI builder outputs against the existing full schemas; no network or production data."""
import copy
import json
import re
import sys
from pathlib import Path
from urllib.parse import urlsplit
from jsonschema import Draft202012Validator, FormatChecker

root = Path(__file__).resolve().parents[2]
directory = Path(sys.argv[1]).resolve(strict=True)
assert str(directory).startswith('/tmp/quota-throttle-sgm6-b-')
schemas = root / 'docs/design/source-groups-token-management/schemas'
checker = FormatChecker()

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
for wire in wires:
    family, definition, value = wire['family'], wire['definition'], wire['value']
    if family in ('request', 'response'):
        schema = json.loads((schemas / f'panel-admin-{family}s.schema.json').read_text())
        schema.pop('anyOf')
        schema['$ref'] = f'#/$defs/{definition}'
    else:
        schema = json.loads((schemas / ('panel-snapshot.schema.json' if family == 'state' else 'panel-ui-config.schema.json')).read_text())
    validator = Draft202012Validator(schema, format_checker=checker)
    validator.validate(value)
    bad = copy.deepcopy(value)
    bad['unexpected_private_field'] = 'synthetic'
    assert list(validator.iter_errors(bad))
    covered.add((family, definition))
assert len(wires) >= 30 and len(covered) == 21, covered
config = json.loads((schemas / 'panel-ui-config.schema.json').read_text())
validator = Draft202012Validator(config, format_checker=checker)
for url in ['http://example.test/v1', 'http://example.test/%56%31/', 'http://example.test/%zz/proxy', 'http://example.test/a|b', 'not a uri', 'http://u:p@example.test']:
    assert list(validator.iter_errors({'schema_version': 1, 'dry_run': False, 'public_api_base_url': url})), url
print(f'Full UI wire schemas: {len(wires)} positives and unknown-field negatives across {len(covered)} families; explicit URI checker and six URL negatives.')
