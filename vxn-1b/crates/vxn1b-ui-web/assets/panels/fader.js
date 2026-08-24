// panels/fader.js — the continuous / fader-family controls: the vertical
// fader, the LFO-rate subdivision label, the rotary waveform knob and the
// bipolar dial, plus the waveform glyph polylines they draw.
//
// `import` lines are dropped by the splice loader (the shared bindings ride
// the bridge slot; the sibling helpers are concatenated in the same scope);
// under Node ESM they resolve so the suites can pull these in via the
// `../panels.js` barrel.
import {
  paintFader, wireFaderDrag, attachValuePop, clampVariant,
  PIXELS_PER_DETENT, KNOB_INDICATOR_TRANSITION_MS,
} from '../util/drag.js';
import { wireDrag } from '../../../../../crates/vxn-core-ui-web/assets/wire-drag.js';

// ─── Waveform glyph polylines ──────────────────────────────────────────────
//
// In a [0, 1]² box (y down). Ported from `wave_points` in
// vxn-ui-vizia/src/lib.rs — coordinates only, no SVG-specific tweaks.
export const WAVE_GLYPHS = {
  'Sine': (() => {
    const pts = [];
    for (let k = 0; k <= 16; k++) {
      const t = k / 16;
      pts.push([t, 0.5 - 0.38 * Math.sin(t * Math.PI * 2)]);
    }
    return pts;
  })(),
  'Triangle': [[0, 0.85], [0.5, 0.15], [1, 0.85]],
  'Tri':      [[0, 0.85], [0.5, 0.15], [1, 0.85]],
  'Saw':      [[0, 0.85], [0.5, 0.15], [0.5, 0.85], [1, 0.15]],
  'Saw+':     [[0, 0.85], [0.5, 0.15], [0.5, 0.85], [1, 0.15]],
  'Saw-':     [[0, 0.15], [0.5, 0.85], [0.5, 0.15], [1, 0.85]],
  'Pulse':    [[0, 0.85], [0, 0.15], [0.5, 0.15], [0.5, 0.85], [1, 0.85]],
  'Square':   [[0, 0.85], [0, 0.15], [0.5, 0.15], [0.5, 0.85], [1, 0.85]],
  'S&H':      [[0, 0.6], [0.28, 0.6], [0.28, 0.2], [0.56, 0.2], [0.56, 0.8], [0.82, 0.8], [0.82, 0.45], [1, 0.45]],
};

export function glyphPath(label, w, h) {
  const pts = WAVE_GLYPHS[label];
  if (!pts) return null;
  return pts.map((p, i) =>
    (i === 0 ? 'M' : 'L') + (p[0] * w).toFixed(2) + ' ' + (p[1] * h).toFixed(2)
  ).join(' ');
}

// ─── Control primitives ────────────────────────────────────────────────────

export function makeFader(el, id, desc, opts) {
  const noLabel = el.hasAttribute('data-no-label');
  const label = el.dataset.label || desc.label;
  const displayOverride = (opts && opts.displayOverride) || null;
  // Optional hooks for faders whose mapping/display swap with a partner
  // toggle (LFO rate ↔ sync, Cutoff ↔ Tuned). `interactionOverride(n)`
  // returns `{plain, norm}` to swap the drag-write path (sends plain Hz
  // instead of raw norm); `normOverride(plain)` returns a thumb norm
  // computed from the param's plain value, bypassing the descriptor
  // taper. Both return null to fall through to the default behaviour.
  const interactionOverride = (opts && opts.interactionOverride) || null;
  const normOverride = (opts && opts.normOverride) || null;
  el.innerHTML = `
    ${noLabel ? '' : `<div class="ctl-label">${label.toUpperCase()}</div>`}
    <div class="ctl-fader">
      <div class="ctl-fader-track"></div>
      <div class="ctl-fader-thumb"></div>
    </div>
  `;
  const fader = el.querySelector('.ctl-fader');
  const thumb = el.querySelector('.ctl-fader-thumb');
  let lastDisplay = '';

  const writeFromDrag = (rawNorm) => {
    const o = interactionOverride && interactionOverride(rawNorm);
    if (o) {
      paintFader(fader, thumb, o.norm);
      window.vxn.send.setParam(id, o.plain);
    } else {
      paintFader(fader, thumb, rawNorm);
      window.vxn.send.setParamNorm(id, rawNorm);
    }
  };

  let drag;
  const pop = attachValuePop({
    isHovered:  () => drag.isHovered(),
    isDragging: () => drag.isDragging(),
  }, () => lastDisplay);
  drag = wireFaderDrag(fader, {
    onEnter: (ev) => pop.markEntered(ev),
    onLeave: () => pop.markLeft(),
    onDown: (ev, n) => {
      window.vxn.send.beginGesture(id);
      writeFromDrag(n);
      pop.markGrabbed(ev);                                // re-anchor at the grab point
    },
    onMove: (_ev, n) => writeFromDrag(n),
    onUp: () => {
      window.vxn.send.endGesture(id);
      pop.markReleased();
    },
  });

  return {
    update(plain, norm, display) {
      // ViewEvent echo — always position the thumb so DAW automation
      // moves it even mid-drag (engine value is authoritative). During a
      // drag the local pointermove `paintFader` and the round-trip echo
      // converge on the same value, so the thumb stays glued to the
      // cursor without flicker.
      const overriddenNorm = normOverride && normOverride(plain);
      paintFader(fader, thumb, overriddenNorm != null ? overriddenNorm : norm);
      // Synced LFO rates swap the Hz readout for a subdivision label
      // (0042). The override is null for every other fader, so this
      // collapses to the plain path.
      let label = display;
      if (displayOverride) {
        const o = displayOverride(plain, norm, display);
        if (o != null) label = o;
      }
      lastDisplay = label;
      pop.refresh();
    },
  };
}

// Map a normalised fader position (linear `[0, 1]`) to the matching
// subdivision label. The LFO rate fader's `norm` is the linear range
// position (`get_normalized`, not the exp-tapered fader-position); since
// `vxn_app::sync::index_from_norm` only ever takes the slider's `0..1`,
// either convention agrees on the index — the table is just spread evenly
// across the travel.
export function subdivisionLabel(norm) {
  const t = window.vxn.subdivisions || [];
  if (t.length === 0) return '';
  const last = t.length - 1;
  const n = Math.max(0, Math.min(1, norm));
  return t[Math.max(0, Math.min(last, Math.round(n * last)))];
}

// ─── Rotary waveform knob ──────────────────────────────────────────────────
//
// Single SVG: knob face + rotating indicator + glyph labels spread around
// a 270° arc with the gap at the bottom (clamped knob, no wrap). Drag
// rotation = vertical pointer motion (up = CW, down = CCW), clamped at
// endpoints, snapped to the nearest detent. Click a glyph for direct
// selection.
//
// Variant angles are evenly distributed across ARC_START..ARC_END, so the
// 4-variant Osc knob still lands its glyphs at SW/NW/NE/SE (the corners
// of -135°…+135° "from up CW") while the 6-variant LFO shape fits without
// crowding the corners. Indicator angle is the same affine function of
// value, so the CSS transition always sweeps along the populated arc.
export const SVG_NS = 'http://www.w3.org/2000/svg';

export function makeWave(el, id, desc) {
  const label = el.dataset.label || desc.label;
  const variants = desc.variants || [];
  el.innerHTML = `<div class="ctl-label">${label.toUpperCase()}</div>`;

  const size = 64;
  const cx = size / 2, cy = size / 2;
  const knobR = 13;
  const glyphR = 26;
  const glyphW = 14, glyphH = 10;

  // 270° arc with a 90° gap at the bottom. Angles measured in degrees CW
  // from "straight up" (0°), so -135° = SW corner, +135° = SE.
  const ARC_START = -135;
  const ARC_SWEEP = 270;
  const N = variants.length;
  const STEP_DEG = N > 1 ? ARC_SWEEP / (N - 1) : 0;
  const variantDeg = (i) => ARC_START + i * STEP_DEG;

  let value = 0;
  let displayedAngle = variantDeg(0);
  let lastDisplay = variants[0] || '';

  const svg = document.createElementNS(SVG_NS, 'svg');
  svg.setAttribute('width', size);
  svg.setAttribute('height', size);
  svg.setAttribute('viewBox', `0 0 ${size} ${size}`);
  svg.classList.add('ctl-wave');
  el.appendChild(svg);

  // Glyph labels along the arc. Transparent rect behind the path makes
  // the whole label area clickable, not just the stroked pixels.
  const glyphEls = variants.map((name, i) => {
    const a = variantDeg(i) * Math.PI / 180;
    const gx = cx + glyphR * Math.sin(a);
    const gy = cy - glyphR * Math.cos(a);
    const g = document.createElementNS(SVG_NS, 'g');
    g.setAttribute('transform',
      `translate(${(gx - glyphW / 2).toFixed(2)} ${(gy - glyphH / 2).toFixed(2)})`);
    g.setAttribute('cursor', 'pointer');

    const hit = document.createElementNS(SVG_NS, 'rect');
    hit.setAttribute('x', -3); hit.setAttribute('y', -3);
    hit.setAttribute('width',  glyphW + 6);
    hit.setAttribute('height', glyphH + 6);
    hit.setAttribute('fill', 'transparent');
    g.appendChild(hit);

    const path = document.createElementNS(SVG_NS, 'path');
    const d = glyphPath(name, glyphW, glyphH);
    if (d) {
      path.setAttribute('d', d);
      path.setAttribute('fill', 'none');
      path.setAttribute('stroke-width', 1.4);
      path.setAttribute('stroke-linecap', 'round');
      path.setAttribute('stroke-linejoin', 'round');
    }
    g.appendChild(path);

    g.addEventListener('pointerdown', (ev) => {
      ev.preventDefault();
      ev.stopPropagation();
      window.vxn.send.discrete(id, i);
    });

    svg.appendChild(g);
    return { g, path, name };
  });

  // Knob face: rim + inner dimple, both purely visual.
  const rim = document.createElementNS(SVG_NS, 'circle');
  rim.setAttribute('cx', cx); rim.setAttribute('cy', cy);
  rim.setAttribute('r', knobR);
  rim.setAttribute('fill', 'var(--knob-face)');
  rim.setAttribute('stroke', 'var(--knob-rim)');
  rim.setAttribute('stroke-width', 1);
  svg.appendChild(rim);

  const dimple = document.createElementNS(SVG_NS, 'circle');
  dimple.setAttribute('cx', cx); dimple.setAttribute('cy', cy);
  dimple.setAttribute('r', knobR * 0.62);
  dimple.setAttribute('fill', 'var(--knob-dimple)');
  dimple.setAttribute('stroke', 'var(--knob-dimple-rim)');
  dimple.setAttribute('stroke-width', 0.5);
  svg.appendChild(dimple);

  // Rotating indicator — a line from centre to rim, rotated by a <g>.
  // CSS transition smooths automation moves between detents.
  const indicatorG = document.createElementNS(SVG_NS, 'g');
  indicatorG.setAttribute('transform-origin', `${cx} ${cy}`);
  indicatorG.style.transition = `transform ${KNOB_INDICATOR_TRANSITION_MS}ms ease-out`;
  const indicator = document.createElementNS(SVG_NS, 'line');
  indicator.setAttribute('x1', cx); indicator.setAttribute('y1', cy);
  indicator.setAttribute('x2', cx); indicator.setAttribute('y2', cy - knobR + 2);
  indicator.setAttribute('stroke', 'var(--knob-indicator)');
  indicator.setAttribute('stroke-width', 2);
  indicator.setAttribute('stroke-linecap', 'round');
  indicatorG.appendChild(indicator);
  svg.appendChild(indicatorG);

  // ── Hover + vertical-drag rotation (no wrap) ───────────────────────────
  // Glyph hits stopPropagation; the knob face falls through to wireDrag.
  // `downContext` stashes the pixel anchor + the value at grab-time so the
  // pointer-to-value map is delta-based, not absolute.
  // `pop` is forward-declared because the drag callbacks reference it but
  // `attachValuePop` needs the drag's hover/drag getters as its host.
  let pop;
  const drag = wireDrag(svg, {
    downContext: (ev) => ({ y0: ev.clientY, v0: value }),
    pointerToValue: (ev, ctx) =>
      clampVariant(ctx.v0 + (ctx.y0 - ev.clientY) / PIXELS_PER_DETENT, variants),
  }, {
    onEnter: (ev) => pop.markEntered(ev),
    onLeave: () => pop.markLeft(),
    onDown:  (ev) => {
      window.vxn.send.beginGesture(id);
      pop.markGrabbed(ev);
    },
    onMove:  (_ev, v) => {
      if (v !== value) window.vxn.send.setParam(id, v);
    },
    onUp:    () => {
      window.vxn.send.endGesture(id);
      pop.markReleased();
    },
  });
  pop = attachValuePop(drag, () => lastDisplay);

  function applyValue(v, display) {
    value = v;
    displayedAngle = variantDeg(v);
    indicatorG.setAttribute('transform', `rotate(${displayedAngle.toFixed(2)})`);
    glyphEls.forEach((g, i) => {
      g.path.setAttribute('stroke',
        i === v ? 'var(--glyph-active)' : 'var(--glyph)');
    });
    lastDisplay = display;
    pop.refresh();
  }

  // Seed the initial pose so the indicator + active-glyph state are right
  // before the first ParamChanged echo lands.
  applyValue(0, variants[0] || '');

  return {
    update(plain, norm, display) {
      const v = clampVariant(plain, variants);
      applyValue(v, display);
    },
  };
}

// ─── Generic rotary dial (0208, Dynamics pane) ─────────────────────────────
//
// Small SVG knob for continuous params, ported from vxn-2's `panels/dial.js`
// into vxn1b's fader-family contract: a 270° arc track + fill + circular face
// + rotating indicator line. Used where panel space is tight — the Dynamics
// pane packs six of these in a 2-row grid instead of six vertical faders.
//
// Interaction is the same gesture / echo contract as `makeFader`, in a rotary
// shape: relative vertical drag on the shared `wireDrag` primitive (rAF-
// throttled, Shift = fine, 200 px full travel), gesture brackets, and the
// hover/drag value-pop. Up (a negative clientY delta) raises the value. No
// double-click numeric entry — vxn1b faders don't have it, so the dial keeps
// parity (drag + hover-pop only).
//
// Geometry mirrors vxn-2's dial: ARC_START = -135°, SWEEP = 270°, a 36 px
// square with the arc outside the dial body so the indicator overprints
// cleanly. `describeArc` walks the SVG arc; `normToDeg` is the same affine
// map so the fill + indicator always agree.
const DIAL_ARC_START = -135;
const DIAL_ARC_SWEEP = 270;
const DIAL_RANGE_PX = 200; // px for full 0..1 travel (matches the fader)
const DIAL_SIZE = 36;
const DIAL_CX = DIAL_SIZE / 2;
const DIAL_CY = DIAL_SIZE / 2;
const DIAL_R = 12;
const DIAL_ARC_R = 15;

function dialNormToDeg(norm) {
  return DIAL_ARC_START + norm * DIAL_ARC_SWEEP;
}

function dialDescribeArc(cx, cy, r, startDeg, endDeg) {
  const s = (startDeg - 90) * Math.PI / 180;
  const e = (endDeg - 90) * Math.PI / 180;
  const sx = cx + r * Math.cos(s);
  const sy = cy + r * Math.sin(s);
  const ex = cx + r * Math.cos(e);
  const ey = cy + r * Math.sin(e);
  const large = (endDeg - startDeg) <= 180 ? 0 : 1;
  return 'M ' + sx.toFixed(2) + ' ' + sy.toFixed(2)
       + ' A ' + r + ' ' + r + ' 0 ' + large + ' 1 '
       + ex.toFixed(2) + ' ' + ey.toFixed(2);
}

export function makeDial(el, id, desc, opts) {
  const noLabel = el.hasAttribute('data-no-label');
  const label = el.dataset.label || desc.label;
  // Same hook as `makeFader`: a dial whose readout is not the raw number
  // (layer pan reads L/C/R, 0248). Null for every other dial, so the plain
  // path is unchanged.
  const displayOverride = (opts && opts.displayOverride) || null;
  // Where the lit arc GROWS FROM. A unipolar param grows from the most
  // anticlockwise point (norm 0 = the bottom of the range); a bipolar one is
  // detented at its centre, so it grows from 12 o'clock in whichever direction
  // the knob was turned — the same read as the bipolar faders' centre-origin
  // fill. Keyed off the descriptor rather than a per-cell flag: a range that
  // straddles zero IS the definition of centre-origin here, and it makes the
  // mixer's pan and detune dials (0248) correct without either cell opting in.
  // `min < 0 && max > 0` deliberately excludes ranges that merely END at zero
  // (dynamics threshold, −60…0 dB), which are unipolar with a negative unit.
  const fillOrigin = (desc.min < 0 && desc.max > 0) ? 0.5 : 0;
  const trackArc = dialDescribeArc(
    DIAL_CX, DIAL_CY, DIAL_ARC_R, DIAL_ARC_START, DIAL_ARC_START + DIAL_ARC_SWEEP);
  el.innerHTML =
    (noLabel ? '' : `<div class="ctl-label">${label.toUpperCase()}</div>`) +
    `<svg class="ctl-dial" width="${DIAL_SIZE}" height="${DIAL_SIZE}" viewBox="0 0 ${DIAL_SIZE} ${DIAL_SIZE}">` +
      `<path class="dial-track" d="${trackArc}" />` +
      `<path class="dial-fill" d="${trackArc}" />` +
      `<circle class="dial-face" cx="${DIAL_CX}" cy="${DIAL_CY}" r="${DIAL_R}" />` +
      `<g class="dial-indicator-g" transform="rotate(0 ${DIAL_CX} ${DIAL_CY})">` +
        `<line class="dial-indicator-line" x1="${DIAL_CX}" y1="${DIAL_CY}" x2="${DIAL_CX}" y2="${DIAL_CY - DIAL_R + 1}" />` +
      `</g>` +
    `</svg>`;

  const svg = el.querySelector('.ctl-dial');
  const fillPath = el.querySelector('.dial-fill');
  const indicatorG = el.querySelector('.dial-indicator-g');
  let currentNorm = 0;
  let lastDisplay = '';

  function paint(norm) {
    currentNorm = norm;
    indicatorG.setAttribute(
      'transform',
      `rotate(${dialNormToDeg(norm).toFixed(2)} ${DIAL_CX} ${DIAL_CY})`);
    // Re-draw the fill arc between `fillOrigin` and `norm` — the span runs
    // whichever way round the two sit, so a bipolar dial fills anticlockwise
    // below centre and clockwise above it. Cap the span at a hair above zero
    // so a norm sitting exactly on the origin still renders a usable
    // zero-length stub (no NaN path).
    const from = Math.min(fillOrigin, norm);
    const to = Math.max(fillOrigin, norm);
    const startDeg = DIAL_ARC_START + from * DIAL_ARC_SWEEP;
    const sweep = Math.max(0.001, to - from) * DIAL_ARC_SWEEP;
    fillPath.setAttribute(
      'd', dialDescribeArc(DIAL_CX, DIAL_CY, DIAL_ARC_R, startDeg, startDeg + sweep));
  }

  const writeFromDrag = (rawNorm) => {
    const n = rawNorm < 0 ? 0 : rawNorm > 1 ? 1 : rawNorm;
    paint(n);
    window.vxn.send.setParamNorm(id, n);
  };

  let drag;
  const pop = attachValuePop({
    isHovered:  () => drag.isHovered(),
    isDragging: () => drag.isDragging(),
  }, () => lastDisplay);
  drag = wireDrag(svg, {
    axis: 'y',
    raf: true,
    downContext: () => ({ startNorm: currentNorm }),
  }, {
    onEnter: (ev) => pop.markEntered(ev),
    onLeave: () => pop.markLeft(),
    onDown: (ev) => {
      window.vxn.send.beginGesture(id);
      writeFromDrag(currentNorm);   // re-anchor at the grab point
      pop.markGrabbed(ev);
    },
    // Up (a negative clientY delta) raises the value.
    onMove: (_ev, info) => writeFromDrag(info.ctx.startNorm - info.dy / DIAL_RANGE_PX),
    onUp: () => {
      window.vxn.send.endGesture(id);
      pop.markReleased();
    },
  });

  // Seed at norm 0; the bind-time echo repaints authoritatively (matches
  // makeFader, which also relies on dispatch pushing an echo on bind).
  paint(0);

  return {
    update(plain, norm, display) {
      // ViewEvent echo — always paint from the authoritative engine norm so
      // DAW automation moves the dial even mid-drag (during a local drag the
      // pointermove paint and the round-trip echo converge on the same value).
      paint(norm);
      let text = display;
      if (displayOverride) {
        const o = displayOverride(plain, norm, display);
        if (o != null) text = o;
      }
      lastDisplay = text;
      pop.refresh();
    },
  };
}

// ─── Bipolar horizontal fader (mod-matrix depth, 0219) ──────────────────────
//
// Ported from vxn-2's `createBipolar` (panels/fader.js): a center-origin
// horizontal fader for a bipolar `[-1, 1]` param — the fill grows *signed* from
// the 50% centre so "same source, opposite depths" reads at a glance (one slot
// +0.5, another −0.5). Follows `makeDial`'s dispatch contract (setParamNorm +
// gesture brackets + an `update` echo), so it automates and rebinds per layer
// like any other cell. The centre of the param's normalised range is 0 depth.
export function makeBipolar(el, id, desc) {
  const noLabel = el.hasAttribute('data-no-label') || el.dataset.label === '';
  const label = el.dataset.label || desc.label;
  el.innerHTML =
    (noLabel ? '' : `<div class="ctl-label">${label.toUpperCase()}</div>`) +
    `<div class="fader-track">` +
      `<div class="vxn-mm-depth-center"></div>` +
      `<div class="fader-track-fill"></div>` +
      `<div class="fader-thumb"></div>` +
    `</div>`;

  const track = el.querySelector('.fader-track');
  const fill = el.querySelector('.fader-track-fill');
  const thumb = el.querySelector('.fader-thumb');
  let currentNorm = 0.5;
  let lastDisplay = '';

  // Signed fill grown horizontally from the 50% centre toward the thumb.
  function paint(norm) {
    currentNorm = norm;
    const pct = norm * 100;
    if (norm >= 0.5) {
      fill.style.left = '50%';
      fill.style.width = `${pct - 50}%`;
    } else {
      fill.style.left = `${pct}%`;
      fill.style.width = `${50 - pct}%`;
    }
    thumb.style.left = `${pct}%`;
  }

  const writeFromDrag = (rawNorm) => {
    const n = rawNorm < 0 ? 0 : rawNorm > 1 ? 1 : rawNorm;
    paint(n);
    window.vxn.send.setParamNorm(id, n);
  };

  let drag;
  const pop = attachValuePop({
    isHovered:  () => drag.isHovered(),
    isDragging: () => drag.isDragging(),
  }, () => lastDisplay);
  // Horizontal relative drag: right (+dx) raises. A wide 400 px full-scale
  // travel (2× vxn-2's) makes gentle depths easy to dial without Shift; Shift
  // still gives 0.1× fine, and double-click resets to 0 (0219 §3 calibration).
  const RANGE_PX = 400;
  drag = wireDrag(track, {
    axis: 'x',
    raf: true,
    downContext: () => ({ startNorm: currentNorm }),
  }, {
    onEnter: (ev) => pop.markEntered(ev),
    onLeave: () => pop.markLeft(),
    onDown: (ev) => {
      window.vxn.send.beginGesture(id);
      writeFromDrag(currentNorm); // re-anchor at the grab point
      pop.markGrabbed(ev);
    },
    onMove: (_ev, info) => writeFromDrag(info.ctx.startNorm + info.dx / RANGE_PX),
    onUp: () => {
      window.vxn.send.endGesture(id);
      pop.markReleased();
    },
  });

  // Seed at centre (0 depth); the bind-time echo repaints authoritatively.
  paint(0.5);

  return {
    update(plain, norm, display) {
      // Always paint from the authoritative engine norm so DAW automation moves
      // the fader even mid-drag (the local paint and the echo converge).
      paint(norm);
      lastDisplay = display;
      pop.refresh();
    },
  };
}
