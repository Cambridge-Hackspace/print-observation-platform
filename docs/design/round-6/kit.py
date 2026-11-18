INK = '#121a14'
P = dict(ground='#1c2620', panel='#263328', panel2='#2f3e31', text='#f6f0de', muted='#b9c4ae',
         sage='#C3D7BB', butter='#F1DB98', coral='#F7B294', cream='#FEF7E4', moss='#9dbb8c',
         dusk='#30364a', far='#2a3a3a', mid='#33483a', hill='#2a3a2e', soil='#202c23', line='#56664f', dim='#9aa892')

CSS = f"""
body{{margin:0}}
a{{color:{P['butter']}}}
button:focus-visible,a:focus-visible{{outline:3px solid {P['butter']};outline-offset:3px}}
.scr{{position:relative;box-sizing:border-box;background:{P['ground']};color:{P['text']};font-family:'Lexend','Segoe UI',system-ui,sans-serif;display:flex;flex-direction:column;overflow:hidden}}
.k{{width:1280px;height:800px;padding:32px 56px 40px;gap:24px}}
.ph{{width:390px;height:844px;padding:22px 20px 28px;gap:16px}}
.ad{{width:1440px;height:900px;padding:28px 40px 36px;gap:22px}}
.panel{{background:{P['panel']};border-radius:6px;box-shadow:6px 6px 0 {INK}}}
.flat{{background:{P['panel']};border-radius:6px}}
.tile{{background:{P['panel2']};border-radius:4px}}
.muted{{color:{P['muted']}}}
.row{{display:flex;align-items:center}}
.col{{display:flex;flex-direction:column}}
.btn{{font:inherit;font-size:20px;font-weight:500;color:{P['text']};background:transparent;border:2px solid {P['line']};border-radius:6px;height:64px;padding:0 24px;white-space:nowrap;cursor:pointer;box-sizing:border-box}}
.btn.s{{background:{P['panel2']};border-color:{P['panel2']}}}
.btn.p{{font-weight:600;color:{INK};background:{P['butter']};border:3px solid {INK};box-shadow:5px 5px 0 {P['coral']}}}
.btn.sm{{font-size:17px;height:52px;padding:0 18px}}
.sq{{display:inline-block;width:10px;height:10px;flex:none}}
.bar{{height:16px;box-sizing:border-box;background:{P['ground']};border:3px solid {INK};border-radius:3px;overflow:hidden}}
.bar>i{{display:block;height:100%}}
.tag{{font-size:14px;padding:4px 9px;border-radius:3px;background:{P['panel2']};white-space:nowrap}}
h1,h2,h3,p{{margin:0}}
"""

def page(title, cls, body, w, h):
    return f'''<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{title}</title>
<script src="./support.js"></script>
</head>
<body>
<x-dc>
<helmet>
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=Lexend:wght@400;500;600&amp;display=swap">
<style>{CSS}</style>
</helmet>
<div class="scr {cls}">
{body}
</div>
</x-dc>
<script type="text/x-dc" data-dc-script data-props='{{"$preview":{{"width":{w},"height":{h}}}}}'>
class Component extends DCLogic {{
  renderVals() {{ return {{}}; }}
}}
</script>
</body>
</html>
'''

def logo(sz=44):
    return f'<svg width="{sz}" height="{sz}" viewBox="0 0 36 36" aria-hidden="true"><circle cx="18" cy="15" r="12" fill="{P["coral"]}" stroke="{INK}" stroke-width="1.8"></circle><circle cx="18" cy="15" r="8.5" fill="none" stroke="{P["butter"]}" stroke-width="1.6"></circle><circle cx="18" cy="15" r="5" fill="{INK}"></circle><rect x="14.5" y="27" width="7" height="4" fill="{P["butter"]}" stroke="{INK}" stroke-width="1.2"></rect><path d="M16 31h4l-2 3z" fill="{P["sage"]}"></path></svg>'

def icon(d, c=None, sz=26, w=2.4):
    c = c or P['text']
    return f'<svg width="{sz}" height="{sz}" viewBox="0 0 24 24" fill="none" stroke="{c}" stroke-width="{w}" stroke-linecap="square" stroke-linejoin="round" aria-hidden="true"><path d="{d}"></path></svg>'

BACK = 'M15 5l-7 7 7 7'
SPROUT = 'M12 21V11M12 14c-5 0-7-3-7-7 5 0 7 3 7 7zM12 11c0-5 3-8 8-8 0 4-3 8-8 8z'
CHECK = 'M5 12l5 5 9-10'
LOCK = 'M6 11h12v9H6zM9 11V8a3 3 0 016 0v3'
WARN = 'M12 4l9 16H3zM12 10v4M12 17v.5'
CAM = 'M3 7h4l2-3h6l2 3h4v12H3zM12 10a3 3 0 100 6 3 3 0 000-6z'
BELL = 'M6 16V11a6 6 0 0112 0v5l2 2H4zM10 20h4'
BOOK = 'M4 5h6a2 2 0 012 2v12a2 2 0 00-2-2H4zM20 5h-6a2 2 0 00-2 2v12a2 2 0 012-2h6z'
SPOOL = 'M5 4h14v4H5zM5 16h14v4H5zM8 8h8v8H8z'
PLUS = 'M12 5v14M5 12h14'
UP = 'M12 19V6M6 11l6-6 6 6M5 21h14'

def kheader(title, back=True, who='Alex', timer=True):
    b = f'<button type="button" class="btn s" style="width:64px;padding:0;display:flex;align-items:center;justify-content:center" aria-label="Back">{icon(BACK, sz=30)}</button>' if back else logo()
    t = f'<span class="muted" style="font-size:16px;text-align:right">Signs you out in 0:45<br>if the screen is left alone</span>' if timer else ''
    return f'''<header class="row" style="justify-content:space-between;gap:24px">
  <div class="row" style="gap:18px">{b}<h1 style="font-size:32px;font-weight:600">{title}</h1></div>
  <div class="row" style="gap:18px">{t}<button type="button" class="btn s">Tag out</button></div>
</header>'''

def pine(x, b, h, fill, ink=False):
    w = h * 0.56
    pts = [(0, -h), (.30, -.66), (.17, -.66), (.42, -.38), (.27, -.38), (.5, -.12), (.07, -.12), (.07, 0)]
    right = [(x + dx * w, b + dy * h) if dy > -1 else (x, b - h) for dx, dy in pts]
    left = [(x - dx * w, b + dy * h) for dx, dy in reversed(pts[1:])]
    d = 'M' + ' L'.join(f'{px:.1f} {py:.1f}' for px, py in right + left) + 'Z'
    s = f' stroke="{INK}" stroke-width="3"' if ink else ''
    return f'<path d="{d}" fill="{fill}"{s}></path>'

def fern(x, b, h, c):
    segs = [f'M{x} {b}V{b-h}']
    for i in range(int(h // 11)):
        y = b - 8 - i * 11; L = max(5, 15 - i * 2)
        segs.append(f'M{x} {y}l{-L} {-L*0.55:.1f}M{x} {y}l{L} {-L*0.55:.1f}')
    return f'<path d="{"".join(segs)}" fill="none" stroke="{c}" stroke-width="3" stroke-linecap="square"></path>'

def shroom(x, b, s, cap):
    st = f'<rect x="{x-s*.28:.1f}" y="{b-s:.1f}" width="{s*.56:.1f}" height="{s:.1f}" fill="{P["cream"]}" stroke="{INK}" stroke-width="3"></rect>'
    cp = f'<path d="M{x-s:.1f} {b-s*.9:.1f}a{s:.1f} {s*.85:.1f} 0 0 1 {2*s:.1f} 0z" fill="{cap}" stroke="{INK}" stroke-width="3" stroke-linejoin="round"></path>'
    q = s * .22
    sp = ''.join(f'<rect x="{x+dx*s-q/2:.1f}" y="{b-s*.9-dy*s-q/2:.1f}" width="{q:.1f}" height="{q:.1f}" fill="{P["cream"]}"></rect>' for dx, dy in [(-.5, .3), (.1, .62), (.5, .28)])
    return st + cp + sp

def pat(pid, a, b):
    return f'<pattern id="{pid}" width="4" height="4" patternUnits="userSpaceOnUse"><rect width="4" height="4" fill="{a}"></rect><rect y="3" width="4" height="1" fill="{b}"></rect></pattern>'

def enclosed(pid, body, nozzle, part=True, screen=None, glass='#26332a'):
    p = f'<rect x="86" y="112" width="48" height="22" fill="url(#{pid})" stroke="{INK}" stroke-width="3"></rect>' if part else ''
    return f'''<rect x="27" y="23" width="180" height="191" rx="4" fill="{INK}" opacity="0.6"></rect>
<rect x="20" y="16" width="180" height="198" rx="4" fill="{body}" stroke="{INK}" stroke-width="4"></rect>
<rect x="38" y="34" width="144" height="122" rx="2" fill="{glass}" stroke="{INK}" stroke-width="4"></rect>
<rect x="46" y="50" width="128" height="9" fill="{P['cream']}" stroke="{INK}" stroke-width="3"></rect>
<rect x="109" y="59" width="2" height="12" fill="{INK}"></rect>
<rect x="96" y="70" width="28" height="22" fill="{nozzle}" stroke="{INK}" stroke-width="3"></rect>
<path d="M104 92h12l-6 9z" fill="{P['cream']}" stroke="{INK}" stroke-width="2.5" stroke-linejoin="round"></path>
{p}<rect x="56" y="134" width="108" height="8" fill="{P['cream']}" stroke="{INK}" stroke-width="3"></rect>
<rect x="38" y="170" width="84" height="28" rx="2" fill="{INK}"></rect>
<rect x="46" y="180" width="44" height="6" fill="{screen or P['butter']}"></rect>
<circle cx="170" cy="184" r="9" fill="{P['cream']}" stroke="{INK}" stroke-width="3"></circle>'''

def openframe(pid, body, nozzle, part=True):
    p = f'<rect x="88" y="150" width="44" height="26" fill="url(#{pid})" stroke="{INK}" stroke-width="3"></rect>' if part else ''
    return f'''<rect x="30" y="20" width="16" height="174" fill="{body}" stroke="{INK}" stroke-width="3"></rect>
<rect x="174" y="20" width="16" height="174" fill="{body}" stroke="{INK}" stroke-width="3"></rect>
<rect x="26" y="10" width="168" height="16" fill="{body}" stroke="{INK}" stroke-width="3"></rect>
<rect x="16" y="190" width="188" height="24" rx="3" fill="{body}" stroke="{INK}" stroke-width="3"></rect>
{p}<rect x="52" y="176" width="116" height="8" fill="{P['cream']}" stroke="{INK}" stroke-width="3"></rect>
<rect x="44" y="90" width="132" height="9" fill="{P['cream']}" stroke="{INK}" stroke-width="3"></rect>
<rect x="96" y="99" width="28" height="22" fill="{nozzle}" stroke="{INK}" stroke-width="3"></rect>
<path d="M104 121h12l-6 8z" fill="{P['cream']}" stroke="{INK}" stroke-width="2.5" stroke-linejoin="round"></path>'''

def printer_svg(kind, pid, body, nozzle, w=120, part=True, label='', extra=''):
    inner = enclosed(pid, body, nozzle, part) if kind == 'e' else openframe(pid, body, nozzle, part)
    return (f'<svg width="{w}" height="{w}" viewBox="0 0 230 230" role="img" aria-label="{label}" style="flex:none">'
            f'<defs>{pat(pid, P["sage"], "#a3bd99")}</defs>{inner}{extra}</svg>')

def spool_svg(color, frac, w=56, label=''):
    # a spool face-on: dark flange, a ring of filament whose thickness shows what's left, the hub
    r_out = 12 + 15 * frac
    return (f'<svg width="{w}" height="{w}" viewBox="0 0 60 60" role="img" aria-label="{label}" style="flex:none">'
            f'<circle cx="30" cy="30" r="27" fill="#3c4a3e" stroke="{INK}" stroke-width="2.5"></circle>'
            f'<circle cx="30" cy="30" r="{r_out:.1f}" fill="{color}" stroke="{INK}" stroke-width="2"></circle>'
            f'<circle cx="30" cy="30" r="11" fill="#3c4a3e" stroke="{INK}" stroke-width="2"></circle>'
            f'<circle cx="30" cy="30" r="5" fill="{P["ground"]}" stroke="{INK}" stroke-width="2"></circle></svg>')

def bar(frac, color=None, h=16):
    color = color or P['sage']
    return f'<div class="bar" style="height:{h}px"><i style="width:{frac*100:.0f}%;background:repeating-linear-gradient(to bottom,{color} 0 3px,#a3bd99 3px 4px)"></i></div>'
