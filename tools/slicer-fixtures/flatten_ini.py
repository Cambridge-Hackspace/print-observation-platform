"""Flatten PrusaSlicer-family vendor presets into one config for `--load`.

The CLI ignores --printer-profile and friends without a datadir the GUI made,
so this resolves each preset's `inherits` chain, as the GUI does, and writes
the values a member choosing those presets would slice with.

    flatten_ini.py VENDOR.ini OUT.ini printer:NAME print:NAME filament:NAME
"""

import re
import sys

path, out, *wanted = sys.argv[1:]
sections, current = {}, None
for line in open(path, encoding="utf-8"):
    line = line.rstrip("\n")
    m = re.match(r"^\[(.+)\]$", line)
    if m:
        current = m.group(1)
        sections[current] = {}
    elif current and "=" in line and not line.lstrip().startswith("#"):
        k, v = line.split("=", 1)
        sections[current][k.strip()] = v.strip()


def resolve(name):
    kind = name.split(":", 1)[0]
    merged = {}
    for parent in filter(None, (p.strip() for p in sections[name].get("inherits", "").split(";"))):
        merged.update(resolve(f"{kind}:{parent}"))
    merged.update(sections[name])
    return merged


# What a preset may be chosen with, not a setting: the CLI would refuse the
# combination on them.
NOT_SETTINGS = {
    "inherits", "renamed_from", "compatible_printers", "compatible_printers_condition",
    "compatible_prints", "compatible_prints_condition",
}
ID = {"printer": "printer_settings_id", "print": "print_settings_id", "filament": "filament_settings_id"}

config = {}
for w in wanted:
    kind, name = w.split(":", 1)
    config.update({k: v for k, v in resolve(w).items() if k not in NOT_SETTINGS})
    config[ID[kind]] = name
with open(out, "w", encoding="utf-8") as f:
    for k, v in config.items():
        f.write(f"{k} = {v}\n")
