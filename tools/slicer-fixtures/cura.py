"""Slice with CuraEngine from the command line.

The engine cannot evaluate the formulas Cura's frontend computes settings
with, so each setting it asks for and was not given is supplied from its
definition's default, one at a time. Which ones were is printed, for the
fixtures' README.

    cura.py CURA_APPDIR MACHINE EXTRUDER MODEL.stl OUT.gcode
"""
import json, os, re, subprocess, sys

appdir, machine, extruder, model, out = sys.argv[1:6]
R = os.path.join(appdir, "share/cura/resources")


def defaults(path, acc):
    j = json.load(open(path))
    if "inherits" in j:
        parent = os.path.join(os.path.dirname(path), j["inherits"] + ".def.json")
        if not os.path.exists(parent):
            parent = os.path.join(R, "definitions", j["inherits"] + ".def.json")
        defaults(parent, acc)

    def walk(s):
        for k, v in s.items():
            if "default_value" in v:
                acc[k] = v["default_value"]
            if "children" in v:
                walk(v["children"])

    walk(j.get("settings", {}))
    for k, v in j.get("overrides", {}).items():
        if "default_value" in v:
            acc[k] = v["default_value"]
    return acc


d = defaults(os.path.join(R, "definitions", machine + ".def.json"), {})
d = defaults(os.path.join(R, "extruders", extruder + ".def.json"), d)
extra = {}
env = dict(os.environ, CURA_ENGINE_SEARCH_PATH=R + "/definitions:" + R + "/extruders")
for _ in range(600):
    args = ["/lib64/ld-linux-x86-64.so.2", "--library-path", appdir, appdir + "/CuraEngine",
            "slice", "-j", f"{R}/definitions/{machine}.def.json"]
    for k, v in extra.items():
        args += ["-s", f"{k}={v}"]
    args += ["-e0", "-j", f"{R}/extruders/{extruder}.def.json"]
    for k, v in extra.items():
        args += ["-s", f"{k}={v}"]
    args += ["-l", model, "-o", out]
    p = subprocess.run(args, env=env, capture_output=True, text=True)
    both = p.stdout + p.stderr
    m = re.search(r"no value given: (\S+)", both)
    if p.returncode == 0:
        print("supplied defaults:", ", ".join(f"{k}={v}" for k, v in extra.items()) or "none")
        sys.exit(0)
    if not m:
        print(both[-800:])
        sys.exit(1)
    k = m.group(1)
    if k in extra or k not in d:
        print("stuck on", k)
        sys.exit(1)
    v = d[k]
    if isinstance(v, bool):
        extra[k] = "true" if v else "false"
    elif isinstance(v, str):
        extra[k] = v
    else:
        extra[k] = json.dumps(v)
print("too many")
sys.exit(1)
