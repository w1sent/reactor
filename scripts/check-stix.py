"""Validate a STIX 2.1 bundle against the OASIS JSON schemas.

A repository script, not part of REactor (ADR-0039: nothing shipped contains Python). It
needs `jsonschema` and the schemas:

    git clone --depth 1 https://github.com/oasis-open/cti-stix2-json-schemas /tmp/stix-schemas
    reactor-gui graph export <case dir> bundle.json
    python3 scripts/check-stix.py /tmp/stix-schemas bundle.json

Every object is checked against the schema of its own type. Custom objects (`x-...`) have
no schema, so only the common properties are asked of them. The schemas' own `bundle.json`
lists the types it knows and leaves out note, opinion, grouping and location, so the
bundle is checked without its `objects`.
"""
import json, pathlib, sys
from jsonschema import Draft202012Validator
from referencing import Registry, Resource
from referencing.jsonschema import DRAFT202012

root = pathlib.Path(sys.argv[1]) / "schemas"
bundle = json.load(open(sys.argv[2]))
resources = []
by_type = {}
for path in root.rglob("*.json"):
    doc = json.load(open(path))
    res = Resource.from_contents(doc, default_specification=DRAFT202012)
    resources.append((doc["$id"], res))
    if path.parent.name in ("sdos", "sros", "observables"):
        by_type[path.stem] = doc
registry = Registry().with_resources(resources)

problems = 0
checked = {}
for obj in bundle["objects"]:
    t = obj["type"]
    schema = by_type.get(t)
    if schema is None:
        # A custom object: the common properties are all the specification asks of it.
        schema = json.load(open(root / "common" / "core.json"))
        label = f"{t} (custom: core properties only)"
    else:
        label = t
    errors = sorted(Draft202012Validator(schema, registry=registry).iter_errors(obj), key=lambda e: list(e.path))
    checked[label] = checked.get(label, 0) + 1
    for e in errors:
        problems += 1
        print(f"{obj['id']}: {'/'.join(map(str, e.path)) or '(object)'}: {e.message[:200]}")
# The bundle schema lists the object types it knows and leaves out note, opinion, grouping
# and location, so its `objects` is not used; each object was checked against its own
# schema above, and the bundle itself is checked here.
bundle_schema = json.load(open(root / "common" / "bundle.json"))
shell = {k: v for k, v in bundle.items() if k != "objects"}
bundle_schema["properties"].pop("objects", None)
for e in Draft202012Validator(bundle_schema, registry=registry).iter_errors(shell):
    problems += 1
    print(f"bundle: {'/'.join(map(str, e.path)) or '(bundle)'}: {e.message[:200]}")
for label, n in sorted(checked.items()):
    print(f"  checked {n:>3} x {label}")
print("PROBLEMS:", problems)
sys.exit(1 if problems else 0)
