"""A 20 x 20 x 2 mm plate as ASCII STL: small, so its G-code is too.

    box.py OUT.stl
"""

import sys

W, D, H = 20, 20, 2
corners = [
    ((0, 0, 0), (0, 1, 0), (1, 1, 0), (1, 0, 0)),
    ((0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)),
    ((0, 0, 0), (1, 0, 0), (1, 0, 1), (0, 0, 1)),
    ((0, 1, 0), (0, 1, 1), (1, 1, 1), (1, 1, 0)),
    ((0, 0, 0), (0, 0, 1), (0, 1, 1), (0, 1, 0)),
    ((1, 0, 0), (1, 1, 0), (1, 1, 1), (1, 0, 1)),
]
lines = ["solid plate"]
for a, b, c, d in corners:
    for tri in ((a, b, c), (a, c, d)):
        lines += [" facet normal 0 0 0", "  outer loop"]
        lines += [f"   vertex {x * W} {y * D} {z * H}" for x, y, z in tri]
        lines += ["  endloop", " endfacet"]
lines.append("endsolid plate")
open(sys.argv[1], "w").write("\n".join(lines) + "\n")
