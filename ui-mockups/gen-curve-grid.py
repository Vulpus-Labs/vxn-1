import math

def pol_direct(v): return v
def pol_bipolar(v): return 2.0*v - 1.0
def pol_abs(v): return abs(v)

def shape_lin(v): return v
def shape_exp(v): return abs(v)*v
def shape_log(v):
    m = math.sqrt(abs(v))
    return -m if v < 0.0 else m

POL = [("Direct", pol_direct, (-1.0, 1.0), "v"),
       ("Bipolar", pol_bipolar, (0.0, 1.0), "2v − 1"),
       ("Abs", pol_abs, (-1.0, 1.0), "|v|")]
SHP = [("Lin", shape_lin, "u"),
       ("Exp", shape_exp, "sign(u)·u²"),
       ("Log", shape_log, "sign(u)·√|u|")]

S = 200          # plot box side px
PAD = 34         # left/bottom label gutter inside each cell
N = 401

def X(x): return (x + 1.0) * 0.5 * S
def Y(y): return (1.0 - (y + 1.0) * 0.5) * S

def poly(fn, a, b):
    pts = []
    for i in range(N):
        x = a + (b - a) * i / (N - 1)
        pts.append("%.2f,%.2f" % (X(x), Y(fn(x))))
    return " ".join(pts)

cells = []
for pi, (pname, pf, (na, nb), pexpr) in enumerate(POL):
    for si, (sname, sf, sexpr) in enumerate(SHP):
        f = lambda x, pf=pf, sf=sf: sf(pf(x))
        code = pi * 3 + si
        native = poly(f, na, nb)
        outside = []
        if na > -1.0: outside.append(poly(f, -1.0, na))
        if nb <  1.0: outside.append(poly(f, nb, 1.0))
        exits = []
        for edge in (na, nb):
            yv = f(edge)
            if abs(yv) > 0.999 and not (edge == -1.0 or edge == 1.0):
                exits.append((X(edge), S if yv < 0 else 0, -1 if yv < 0 else 1))
        cells.append(dict(band=(X(na), X(nb) - X(na)), exits=exits,
                          pname=pname, sname=sname, code=code,
                          expr="u = %s,  y = %s" % (pexpr, sexpr.replace("u", "u")),
                          native=native, outside=outside,
                          nat_dom="v ∈ [%g, %g]" % (na, nb)))

def svg_cell(c):
    grid = []
    for t in (-0.5, 0.5):
        grid.append('<line class="gm" x1="%.1f" y1="0" x2="%.1f" y2="%d"/>' % (X(t), X(t), S))
        grid.append('<line class="gm" x1="0" y1="%.1f" x2="%d" y2="%.1f"/>' % (Y(t), S, Y(t)))
    marks = "".join(
        '<path class="exit" d="M %.1f %.1f l -5 %d l 10 0 z"/>' % (mx, my + (-1 if d < 0 else 1) * 1, -6 * d)
        for mx, my, d in c["exits"])
    ticks = ''.join(
        '<text class="tk" x="%.1f" y="%d" text-anchor="%s">%s</text>' % (X(t), S + 13, a, lb)
        for t, lb, a in ((-1.0, "\u22121", "start"), (0.0, "0", "middle"), (1.0, "1", "end"))
    ) + ''.join(
        '<text class="tk" x="-4" y="%.1f" text-anchor="end" dominant-baseline="middle">%s</text>' % (Y(t), lb)
        for t, lb in ((1.0, "1"), (0.0, "0"), (-1.0, "\u22121")))
    out = "".join(
        '<polyline class="curve out" points="%s"/>' % p for p in c["outside"])
    return """
<figure class="cell">
  <figcaption><span class="nm">%s &middot; %s</span><span class="code">code %d</span></figcaption>
  <svg viewBox="-20 -6 %d %d" role="img" aria-label="%s %s mapping">
    <clipPath id="clip%d"><rect x="0" y="0" width="%d" height="%d"/></clipPath>
    <rect class="box" x="0" y="0" width="%d" height="%d"/>
    <rect class="band" x="%.1f" y="0" width="%.1f" height="%d"/>
    %s
    <line class="ax" x1="%.1f" y1="0" x2="%.1f" y2="%d"/>
    <line class="ax" x1="0" y1="%.1f" x2="%d" y2="%.1f"/>
    <g clip-path="url(#clip%d)">
      <polyline class="ident" points="%s"/>
      %s
      <polyline class="curve" points="%s"/>
    </g>
    %s
    %s
  </svg>
  <p class="expr">%s</p>
  <p class="dom">native input %s</p>
</figure>""" % (c["pname"], c["sname"], c["code"], S + 26, S + 26,
                c["pname"], c["sname"], c["code"], S, S, S, S,
                c["band"][0], c["band"][1], S,
                "".join(grid), X(0), X(0), S, Y(0), S, Y(0), c["code"],
                poly(lambda x: x, -1.0, 1.0), out, c["native"], marks, ticks,
                c["expr"], c["nat_dom"])

TEMPLATE = """<title>Mod Matrix Curve Grid</title>
<style>
  :root {
    --bg:#faf9f7; --fg:#1c1b19; --dim:#6f6b66; --line:#d9d4cd;
    --grid:#ebe7e1; --curve:#c2410c; --ident:#c9c3ba; --box:#ffffff; --band:#efe6d8;
  }
  :root:not([data-theme="light"]) {}
  @media (prefers-color-scheme: dark) {
    :root:not([data-theme="light"]) {
      --bg:#16151a; --fg:#eceaf0; --dim:#9b96a3; --line:#35323d;
      --grid:#26242c; --curve:#fb923c; --ident:#454150; --box:#1d1c22; --band:#2b2833;
    }
  }
  :root[data-theme="dark"] {
    --bg:#16151a; --fg:#eceaf0; --dim:#9b96a3; --line:#35323d;
    --grid:#26242c; --curve:#fb923c; --ident:#454150; --box:#1d1c22; --band:#2b2833;
  }
  body { background:var(--bg); color:var(--fg); margin:0; padding:2.5rem 1.5rem 4rem;
         font:15px/1.5 ui-sans-serif,-apple-system,"Segoe UI",sans-serif; }
  .wrap { max-width:980px; margin:0 auto; }
  h1 { font-size:1.45rem; margin:0 0 .35rem; letter-spacing:-.01em; }
  .sub { color:var(--dim); margin:0 0 2rem; max-width:62ch; }
  .grid { display:grid; grid-template-columns:repeat(3,minmax(0,1fr)); gap:1.5rem; }
  @media (max-width:760px){ .grid{ grid-template-columns:repeat(2,minmax(0,1fr)); } }
  @media (max-width:520px){ .grid{ grid-template-columns:1fr; } }
  .cell { margin:0; }
  figcaption { display:flex; justify-content:space-between; align-items:baseline;
               margin-bottom:.4rem; }
  .nm { font-weight:600; letter-spacing:-.01em; }
  .code { color:var(--dim); font-size:.72rem; font-variant-numeric:tabular-nums; }
  svg { width:100%; height:auto; display:block; }
  .band { fill:var(--band); }
  .tk { fill:var(--dim); font:10px ui-monospace,SFMono-Regular,Menlo,monospace; }
  .exit { fill:var(--curve); opacity:.8; }
  .box { fill:var(--box); stroke:var(--line); stroke-width:1; }
  .gm { stroke:var(--grid); stroke-width:1; }
  .ax { stroke:var(--line); stroke-width:1; }
  .ident { fill:none; stroke:var(--ident); stroke-width:1.25; stroke-dasharray:2 3; }
  .curve { fill:none; stroke:var(--curve); stroke-width:2.5;
           stroke-linecap:round; stroke-linejoin:round; }
  .curve.out { stroke-width:1.75; stroke-dasharray:4 4; opacity:.55; }
  .expr { margin:.5rem 0 .1rem; font:12.5px/1.4 ui-monospace,SFMono-Regular,Menlo,monospace;
          color:var(--fg); }
  .dom { margin:0; font-size:.75rem; color:var(--dim); }
  .key { margin-top:2.25rem; padding-top:1.25rem; border-top:1px solid var(--line);
         color:var(--dim); font-size:.85rem; max-width:70ch; }
  .key b { color:var(--fg); font-weight:600; }
</style>
<div class="wrap">
  <h1>Mod matrix curve grid</h1>
  <p class="sub">All nine <code>(polarity, shape)</code> combinations, plotted from
  <code>vxn_core_matrix::curve</code>. Polarity runs first, the shape bend second:
  <code>y = bend(shape, polarity(v))</code>. Axes span [&minus;1, 1] in both
  directions; the dotted diagonal is identity for reference.</p>
  <div class="grid">
@@CELLS@@
  </div>
  <p class="key"><b>Solid</b> = the source range that polarity is written for
  (<i>Bipolar</i> expects a unipolar [0, 1] source; <i>Abs</i> and <i>Direct</i>
  take the full [&minus;1, 1]). <b>Dashed orange</b> = the same formula continued
  outside that range, clipped at the box edge &mdash; that is what a source of the
  &ldquo;wrong&rdquo; polarity actually produces, not a clamp. <b>Codes</b> are the flat
  <code>curve_code = polarity&nbsp;&times;&nbsp;3 + shape</code> preset files carry.</p>
</div>
"""
html = TEMPLATE.replace("@@CELLS@@", "\n".join(svg_cell(c) for c in cells))

open("/private/tmp/claude-501/-Users-dominicfox-src-vxn-1/7b73687c-bdca-4da9-8b58-4fbfab83a1ca/scratchpad/curve-grid.html", "w").write(html)
print("ok", len(html))
