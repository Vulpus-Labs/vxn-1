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

  // Vertical pointer travel (px) per variant step when dragging the knob
  // face. ~28 matches the hardware-knob feel the vxn-1 port used.
  const PIXELS_PER_DETENT = 28;

  function create(el, ctx) {
    const desc = ctx.desc;
    const variants = (desc && desc.kind === "enum" && desc.variants) ? desc.variants : [];
    const n = variants.length;
    let currentIdx = Math.round(desc ? desc.default : 0) | 0;
    if (currentIdx < 0) currentIdx = 0;
    if (currentIdx >= n) currentIdx = n - 1;

    const SIZE = 64, CX = SIZE / 2, CY = SIZE / 2;
    const ARC_START = -135, ARC_SWEEP = 270;
    const stepDeg = n > 1 ? ARC_SWEEP / (n - 1) : 0;
    const variantDeg = function (i) { return ARC_START + i * stepDeg; };

    let glyphPaths = [];
    let indicatorG = null;
    let dragging = false;

    function clampIdx(i) { return Math.max(0, Math.min(n - 1, i | 0)); }

    // Build the SVG structure once. Value changes (click / drag / host echo)
    // mutate attributes via applyValue — never a full innerHTML rebuild, so
    // an in-flight drag keeps its pointer capture on a live <svg> node.
    function build() {
      render(el, variants, currentIdx);
      const groups = el.querySelectorAll("g[data-variant]");
      glyphPaths = [];
      for (let i = 0; i < groups.length; i++) {
        glyphPaths[i] = groups[i].querySelector(".wave-glyph");
      }
      indicatorG = el.querySelector(".knob-indicator-g");
      bindGlyphClicks(groups);
      bindDrag();
    }

    // Cheap value application — rotate the indicator and re-mark the active
    // glyph. No DOM teardown.
    function applyValue(idx) {
      currentIdx = idx;
      if (indicatorG) {
        indicatorG.setAttribute(
          "transform",
          "rotate(" + variantDeg(idx).toFixed(2) + " " + CX + " " + CY + ")"
        );
      }
      for (let i = 0; i < glyphPaths.length; i++) {
        if (!glyphPaths[i]) continue;
        glyphPaths[i].setAttribute("class", i === idx ? "wave-glyph active" : "wave-glyph");
      }
    }

    function commit(idx) {
      if (idx === currentIdx) return;
      ctx.setParam(idx);
      applyValue(idx); // optimistic; host echo confirms via set()
    }

    function bindGlyphClicks(groups) {
      for (let i = 0; i < groups.length; i++) {
        const g = groups[i];
        g.addEventListener("click", function (ev) {
          ev.preventDefault();
          commit(parseInt(g.getAttribute("data-variant"), 10));
        });
      }
    }

    // Rotary drag on the knob face: grab anywhere that isn't a glyph hit
    // (glyphs keep their direct click-to-select) and drag vertically to step
    // through variants. Up = next, no wrap, gesture-bracketed.
    function bindDrag() {
      const svg = el.querySelector("svg");
      if (!svg) return;
      let startY = 0, startIdx = 0;
      svg.addEventListener("pointerdown", function (ev) {
        // A press that lands on a glyph is a click-to-select, not a drag.
        if (ev.target instanceof Element && ev.target.closest("g[data-variant]")) {
          return;
        }
        ev.preventDefault();
        dragging = true;
        startY = ev.clientY;
        startIdx = currentIdx;
        if (svg.setPointerCapture) {
          try { svg.setPointerCapture(ev.pointerId); } catch (_) {}
        }
        ctx.beginGesture();
      });
      svg.addEventListener("pointermove", function (ev) {
        if (!dragging) return;
        ev.preventDefault();
        const sens = ev.shiftKey ? 0.25 : 1.0;
        const steps = Math.round(((startY - ev.clientY) * sens) / PIXELS_PER_DETENT);
        commit(clampIdx(startIdx + steps));
      });
      function up(ev) {
        if (!dragging) return;
        ev.preventDefault();
        dragging = false;
        if (svg.releasePointerCapture) {
          try { svg.releasePointerCapture(ev.pointerId); } catch (_) {}
        }
        ctx.endGesture();
      }
      svg.addEventListener("pointerup", up);
      svg.addEventListener("pointercancel", up);
      svg.addEventListener("dblclick", function (ev) {
        ev.preventDefault();
        ctx.requestTextInput();
      });
    }

    build();

    return {
      set: function (plain) {
        // Don't let a host echo stomp the value mid-drag (the page is the
        // source of truth while the user is turning the knob).
        if (dragging) return;
        applyValue(clampIdx(Math.round(plain)));
      },
    };
  }

  window.__vxn.panels.knob = { create: create };
})();
