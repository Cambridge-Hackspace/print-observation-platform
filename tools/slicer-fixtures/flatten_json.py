"""Flatten a Bambu Studio or OrcaSlicer system preset for `--load-settings`.

Resolves `inherits` by name across the vendor folders, as the GUI does, so
the CLI slices with the values a member choosing that preset would have.
Loaded unflattened, a leaf preset loses what it inherits: a Bambu filament
then has density 0 and the file says it weighs 0 g.

    flatten_json.py VENDOR_DIR[:VENDOR_DIR...] machine|process|filament NAME OUT.json
"""

import json
import os
import sys

roots, kind, name, out = sys.argv[1:5]
presets = {}
for root in roots.split(":"):
    for d, _, files in os.walk(os.path.join(root, kind)):
        for f in files:
            if f.endswith(".json"):
                j = json.load(open(os.path.join(d, f), encoding="utf-8"))
                if "name" in j:
                    presets[j["name"]] = j


def resolve(n):
    merged = {}
    if presets[n].get("inherits"):
        merged.update(resolve(presets[n]["inherits"]))
    merged.update(presets[n])
    return merged


p = resolve(name)
for k in ("inherits", "instantiation", "setting_id", "base_id"):
    p.pop(k, None)
# "system", so the CLI checks the process against the printer by these names,
# as it would for the presets the GUI offers.
p["from"] = "system"
p["name"] = name
json.dump(p, open(out, "w", encoding="utf-8"), indent=1)
