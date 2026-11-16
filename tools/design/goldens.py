#!/usr/bin/env python3
"""Draws pieces of the round 6 kit into web/src/lib/design/golden.json, for
the design system's tests to compare the components with (#48).

    python3 tools/design/goldens.py
"""
import json, os, sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
sys.path.insert(0, os.path.join(ROOT, 'docs', 'design', 'round-6'))
import kit  # noqa: E402

def svg(inner, vb='0 0 240 240'):
    return f'<svg viewBox="{vb}">{inner}</svg>'

goldens = {
    'palette': {**kit.P, 'ink': kit.INK},
    'printer': [
        {'args': {'kind': k, 'body': body, 'nozzle': nozzle, 'part': part},
         'svg': kit.printer_svg(k, 'g', body, nozzle, w=120, part=part, label='X1')}
        for k, body, nozzle, part in [
            ('e', kit.P['coral'], kit.P['butter'], True),
            ('e', '#7fb6d9', kit.P['cream'], False),
            ('o', kit.P['sage'], kit.P['coral'], True),
            ('o', '#c9a0dc', kit.P['butter'], False),
        ]
    ],
    'spool': [
        {'args': {'color': c, 'frac': f}, 'svg': kit.spool_svg(c, f, w=56, label='PLA')}
        for c, f in [(kit.P['coral'], 1.0), ('#7fb6d9', 0.25), (kit.P['sage'], 0.0)]
    ],
    'pine': [
        {'args': {'x': x, 'b': b, 'h': h, 'fill': f, 'ink': ink}, 'svg': svg(kit.pine(x, b, h, f, ink))}
        for x, b, h, f, ink in [(120, 230, 150, kit.P['far'], False), (60, 200, 90, kit.P['sage'], True)]
    ],
    'fern': [
        {'args': {'x': x, 'b': b, 'h': h, 'color': c}, 'svg': svg(kit.fern(x, b, h, c))}
        # The tall one reaches the shortest fronds, which only a fern over
        # 77 high has.
        for x, b, h, c in [(116, 230, 70, kit.P['sage']), (40, 200, 34, kit.P['mid']), (180, 230, 120, kit.P['moss'])]
    ],
    'shroom': [
        {'args': {'x': x, 'b': b, 's': s, 'cap': c}, 'svg': svg(kit.shroom(x, b, s, c))}
        for x, b, s, c in [(120, 230, 22, kit.P['coral']), (60, 200, 14, kit.P['butter'])]
    ],
}
out = os.path.join(ROOT, 'web', 'src', 'lib', 'design', 'golden.json')
os.makedirs(os.path.dirname(out), exist_ok=True)
with open(out, 'w') as f:
    json.dump(goldens, f, indent=1)
    f.write('\n')
print(f'wrote {out}')
