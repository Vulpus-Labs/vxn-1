// VXN2 generic rotary dial — small SVG knob for continuous float / int params.
//
// Vertical drag → throttled `set_norm` per animation frame → `end_gesture` on
// pointer-up. Shift drag = 1/10 sensitivity. Double-click pops the native
// numeric-entry popup. Shares the value-pop singleton + taper math with
// `panels/fader.js`; the dial is the same gesture / echo contract in a
// rotary shape, used where panel space is tight (E028 Dynamics pane).
//
// Bound via `.dial[data-vxn-param]` markup; `main.js`'s `bindDials` constructs
// it through `window.__vxn.panels.dial.create(el, ctx)`.

(function () {
  // Shared floating value popup (0140): one `valuePop` singleton (spliced
  // ahead of this module) backs every control, so a fader and a dial share
  // the one `.value-pop` node. Thin aliases keep the call sites unchanged.
  function showPop(text, x, y) { valuePop.show(text, x, y); }
  function updatePop(text) { valuePop.update(text); }
  function hidePop() { valuePop.hide(); }

  // Reuse the fader's taper math so a dial-bound param and a fader-bound param
  // round-trip the same way through the host. The fader panel registers itself
  // on `window.__vxn.panels.fader` (load order is bootstrap → knob → fader →
  // ... in lib.rs), so by the time a `.dial` is bound the helpers are present.
  function paramToNorm(desc, plain) {
    const fader = window.__vxn && window.__vxn.panels && window.__vxn.panels.fader;
    return fader ? fader.paramToNorm(desc, plain) : 0;
  }
  function normToParam(desc, norm) {
    const fader = window.__vxn && window.__vxn.panels && window.__vxn.panels.fader;
    return fader ? fader.normToParam(desc, norm) : 0;
  }
  function formatDisplay(desc, plain) {
    const fader = window.__vxn && window.__vxn.panels && window.__vxn.panels.fader;
    return fader ? fader.formatDisplay(desc, plain) : String(plain);
  }

  // 270° arc, indicator points up at norm=0.5 (12 o'clock).
  const ARC_START_DEG = -135;
  const ARC_SWEEP_DEG = 270;
  function normToDeg(norm) {
    return ARC_START_DEG + norm * ARC_SWEEP_DEG;
  }

  // SVG geometry. Small footprint: 36 px square, dial radius 12, indicator
  // line from centre to just inside the rim. The arc lives outside the dial
  // body so the indicator overprints cleanly.
  const SIZE = 36;
  const CX = SIZE / 2;
  const CY = SIZE / 2;
  const DIAL_R = 12;
  const ARC_R = 15;

  function describeArc(cx, cy, r, startDeg, endDeg) {
    const s = (startDeg - 90) * Math.PI / 180;
    const e = (endDeg - 90) * Math.PI / 180;
    const sx = cx + r * Math.cos(s);
    const sy = cy + r * Math.sin(s);
    const ex = cx + r * Math.cos(e);
    const ey = cy + r * Math.sin(e);
    const large = (endDeg - startDeg) <= 180 ? 0 : 1;
    return "M " + sx.toFixed(2) + " " + sy.toFixed(2)
         + " A " + r + " " + r + " 0 " + large + " 1 "
         + ex.toFixed(2) + " " + ey.toFixed(2);
  }

  function render(el, label) {
    const trackArc = describeArc(CX, CY, ARC_R, ARC_START_DEG, ARC_START_DEG + ARC_SWEEP_DEG);
    let html = "";
    if (label) html += '<div class="dial-label">' + label + "</div>";
    html += '<svg class="dial-svg" width="' + SIZE + '" height="' + SIZE
         + '" viewBox="0 0 ' + SIZE + " " + SIZE + '">';
    html += '<path class="dial-track" d="' + trackArc + '" />';
    html += '<path class="dial-fill" d="' + trackArc + '" />';
    html += '<circle class="dial-face" cx="' + CX + '" cy="' + CY + '" r="' + DIAL_R + '" />';
    html += '<g class="dial-indicator-g" transform="rotate(0 ' + CX + " " + CY + ')">';
    html += '<line class="dial-indicator-line" x1="' + CX + '" y1="' + CY
         + '" x2="' + CX + '" y2="' + (CY - DIAL_R + 1) + '" />';
    html += "</g></svg>";
    el.innerHTML = html;
  }

  function labelOf(el, desc) {
    const explicit = el.getAttribute("data-label");
    if (explicit) return explicit;
    // Fall back to the short half of the descriptor's display name ("Dyn
    // Threshold" → "Threshold", "Dyn Mix" → "Mix"). Authors can override
    // entirely with `data-label`.
    if (desc && desc.label) {
      const parts = desc.label.split(" ");
      return parts.length > 1 ? parts.slice(1).join(" ") : desc.label;
    }
    return "";
  }

  function create(el, ctx) {
    const desc = ctx.desc;
    if (!desc) return { set: function () {} };

    render(el, labelOf(el, desc));

    const fillPath = el.querySelector(".dial-fill");
    const indicatorG = el.querySelector(".dial-indicator-g");
    let currentPlain = desc.default;
    let currentNorm = paramToNorm(desc, currentPlain);

    function displayText() { return formatDisplay(desc, currentPlain); }

    function paint() {
      if (indicatorG) {
        indicatorG.setAttribute(
          "transform",
          "rotate(" + normToDeg(currentNorm).toFixed(2) + " " + CX + " " + CY + ")"
        );
      }
      if (fillPath) {
        // Re-draw the fill arc up to the current norm; cap at >= ~0 so a
        // norm of 0 still renders a usable zero-length stub (no NaN path).
        const sweep = Math.max(0.001, currentNorm) * ARC_SWEEP_DEG;
        fillPath.setAttribute(
          "d",
          describeArc(CX, CY, ARC_R, ARC_START_DEG, ARC_START_DEG + sweep)
        );
      }
      if (popActive()) updatePop(displayText());
    }

    // Drag handle (assigned below); `null` until then so `popActive()` — called
    // by the first `paint()` — reads false instead of tripping the TDZ.
    let drag = null;
    function popActive() {
      return drag && (drag.isHovered() || drag.isDragging());
    }

    paint();

    function postNorm(n) {
      const clamped = n < 0 ? 0 : n > 1 ? 1 : n;
      ctx.setNorm(clamped);
      currentNorm = clamped;
      currentPlain = normToParam(desc, clamped);
      paint();
    }

    // ── Gesture ── vertical relative drag on the shared wireDrag primitive
    // (0140): the same contract as the fader (rAF-throttled, 0.1× shift-fine,
    // 200 px full travel, `dataset.dragging` echo-gate, gesture brackets,
    // value-pop) in a rotary shape.
    const RANGE_PX = 200; // px for full 0..1 travel (matches the fader)
    drag = wireDrag(el, {
      axis: "y",
      raf: true,
      downContext: () => ({ startNorm: currentNorm }),
    }, {
      onEnter: (ev) => showPop(displayText(), ev.clientX, ev.clientY),
      onDown: (ev) => {
        el.dataset.dragging = "1";
        showPop(displayText(), ev.clientX, ev.clientY);
        ctx.beginGesture();
      },
      // Up (a negative clientY delta) raises the value.
      onMove: (_ev, info) => postNorm(info.ctx.startNorm - info.dy / RANGE_PX),
      onUp: () => {
        delete el.dataset.dragging;
        ctx.endGesture();
      },
      onLeave: () => hidePop(),
      onDoubleClick: () => ctx.requestTextInput(formatDisplay(desc, currentPlain)),
    });

    return {
      set: function (plain) {
        if (drag.isDragging()) return; // user gesture wins over host echoes
        currentPlain = plain;
        currentNorm = paramToNorm(desc, plain);
        paint();
      },
    };
  }

  window.__vxn = window.__vxn || {};
  window.__vxn.panels = window.__vxn.panels || {};
  window.__vxn.panels.dial = { create: create };
})();
