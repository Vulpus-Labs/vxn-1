// VXN2 wave-shape knob (port of vxn-1 panels.js / ui-mockup `.wave-knob`).
// Binds to enum params whose variants match the glyph table — in 0026,
// `lfo1-shape` and `lfo2-shape` (LFO_SHAPES: Sine, Tri, Saw+, Saw-,
// Pulse, S&H). Click a glyph to select; double-click the centre for the
// native numeric-entry popup.

(function () {
  const WAVE_GLYPHS = {
    "Sine": (function () {
      const pts = [];
      for (let k = 0; k <= 16; k++) {
        const t = k / 16;
        pts.push([t, 0.5 - 0.38 * Math.sin(t * Math.PI * 2)]);
      }
      return pts;
    })(),
    "Tri":   [[0, 0.85], [0.5, 0.15], [1, 0.85]],
    "Saw+":  [[0, 0.85], [0.5, 0.15], [0.5, 0.85], [1, 0.15]],
    "Saw-":  [[0, 0.15], [0.5, 0.85], [0.5, 0.15], [1, 0.85]],
    "Pulse": [[0, 0.85], [0, 0.15], [0.5, 0.15], [0.5, 0.85], [1, 0.85]],
    "S&H":   [[0, 0.6], [0.28, 0.6], [0.28, 0.2], [0.56, 0.2],
              [0.56, 0.8], [0.82, 0.8], [0.82, 0.45], [1, 0.45]],
  };

  function glyphPath(name, w, h) {
    const pts = WAVE_GLYPHS[name];
    if (!pts) return null;
    return pts.map(function (p, i) {
      return (i === 0 ? "M" : "L") + (p[0] * w).toFixed(2) + " " + (p[1] * h).toFixed(2);
    }).join(" ");
  }

  function render(el, variants, currentIdx) {
    const size = 64, cx = size / 2, cy = size / 2;
    const knobR = 13, glyphR = 26, glyphW = 14, glyphH = 10;
    const ARC_START = -135, ARC_SWEEP = 270;
    const n = variants.length;
    const stepDeg = n > 1 ? ARC_SWEEP / (n - 1) : 0;
    const variantDeg = function (i) { return ARC_START + i * stepDeg; };

    let svg = '<svg width="' + size + '" height="' + size + '" viewBox="0 0 ' + size + ' ' + size + '">';
    svg += '<circle class="knob-face" cx="' + cx + '" cy="' + cy + '" r="' + knobR + '" />';
    svg += '<circle class="knob-dimple" cx="' + cx + '" cy="' + cy + '" r="' + (knobR * 0.62).toFixed(2) + '" />';
    for (let i = 0; i < n; i++) {
      const a = variantDeg(i) * Math.PI / 180;
      const gx = cx + glyphR * Math.sin(a);
      const gy = cy - glyphR * Math.cos(a);
      const tx = (gx - glyphW / 2).toFixed(2);
      const ty = (gy - glyphH / 2).toFixed(2);
      const d = glyphPath(variants[i], glyphW, glyphH);
      const cls = i === currentIdx ? "wave-glyph active" : "wave-glyph";
      svg += '<g transform="translate(' + tx + ' ' + ty + ')" data-variant="' + i + '">';
      svg += '<rect class="wave-hit" x="-3" y="-3" width="' + (glyphW + 6) + '" height="' + (glyphH + 6) + '" />';
      if (d) svg += '<path class="' + cls + '" d="' + d + '" />';
      svg += '</g>';
    }
    const ang = variantDeg(currentIdx).toFixed(2);
    svg += '<g class="knob-indicator-g" transform="rotate(' + ang + ' ' + cx + ' ' + cy + ')">';
    svg += '<line class="knob-indicator-line" x1="' + cx + '" y1="' + cy + '" x2="' + cx + '" y2="' + (cy - knobR + 2) + '" />';
    svg += '</g>';
    svg += '</svg>';

    el.innerHTML = '<div class="wave-knob-label">Shape</div>' + svg;
  }

  function create(el, ctx) {
    const desc = ctx.desc;
    const variants = (desc && desc.kind === "enum" && desc.variants) ? desc.variants : [];
    let currentIdx = Math.round(desc ? desc.default : 0) | 0;
    if (currentIdx < 0) currentIdx = 0;
    if (currentIdx >= variants.length) currentIdx = variants.length - 1;

    function paint() { render(el, variants, currentIdx); bindClicks(); }

    function bindClicks() {
      const glyphs = el.querySelectorAll("g[data-variant]");
      for (let i = 0; i < glyphs.length; i++) {
        const g = glyphs[i];
        g.addEventListener("click", function (ev) {
          ev.preventDefault();
          const idx = parseInt(g.getAttribute("data-variant"), 10);
          if (idx === currentIdx) return;
          ctx.setParam(idx);
          // Optimistic local update; host echo will confirm via set().
          currentIdx = idx;
          paint();
        });
      }
      const svg = el.querySelector("svg");
      if (svg) {
        svg.addEventListener("dblclick", function (ev) {
          ev.preventDefault();
          ctx.requestTextInput();
        });
      }
    }

    paint();

    return {
      set: function (plain) {
        const idx = Math.max(0, Math.min(variants.length - 1, Math.round(plain) | 0));
        if (idx === currentIdx) return;
        currentIdx = idx;
        paint();
      },
    };
  }

  window.__vxn.panels.knob = { create: create };
})();
