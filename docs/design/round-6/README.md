# Round 6: the approved look

`kit.py` is the generator the approved round 6 canvas was drawn with: the
palette, the type, and every drawing (printers, spools, the forest). It is
the source of truth for the web app's design system (#48), in
`web/src/lib/design`.

`tools/design/goldens.py` draws a set of pieces with it into
`web/src/lib/design/golden.json`, and the design system's tests compare each
component's SVG with its golden, element by element and attribute by
attribute. A drawing that drifts from the approved look fails a test. Change
the look here first, regenerate the goldens, then the components.
