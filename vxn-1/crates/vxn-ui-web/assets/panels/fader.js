// panels/fader.js — the continuous / fader-family controls: the vertical
// fader, the LFO-rate subdivision label, the rotary waveform knob, and the
// Detune+Legato composite, plus the waveform glyph polylines they draw.
// Split out of the panels.js god-file in ticket 0141.
//
// `import` lines are dropped by the splice loader (the shared bindings ride
// the bridge slot; the sibling helpers are concatenated in the same scope);
// under Node ESM they resolve so the suites can pull these in via the
// `../panels.js` barrel. `variantIdx` is a concat-global from dispatch.js
// (referenced only inside `makeDetuneLegato`, at editor-ready time).
import {
  paintFader, wireFaderDrag, attachValuePop, clampVariant,
  PIXELS_PER_DETENT, KNOB_INDICATOR_TRANSITION_MS, tgRow,
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

// Detune ceiling in Twin assign mode (cents). Twin's "useful" range is
// purely a view convention — the engine doesn't enforce it, so the
// editor that surfaces the mode is the one that has to clamp. Mirrors
// vxn_ui_vizia::TWIN_DETUNE_CT (retired in 0054 but the value is still
// load-bearing).
export const TWIN_TOP_CT = 20.0;

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
//
// **Future**: when intermediate / cross-fade waveforms ship, this becomes
// a continuous `[0, N)` knob with wrap-around. The angle math already
// works for fractional values; only the drag clamp + glyph-active logic
// need a `wrap: true` branch.
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

// ─── Detune + Legato composite (Voice panel, 0045) ─────────────────────────
//
// Two params + one watch in a single column: the Detune fader on top and
// the Legato toggle beneath it, both driven by Assign Mode for visual hints
// (dim Legato in Poly/Twin) and behaviour (Detune fader's full-travel
// meaning is 50 ct in Unison vs 20 ct in Twin — mirrors
// `vxn_ui_vizia::detune_top`). Plain values stay in descriptor units (0–50
// ct); only the fader's [0,1] → cents map changes per mode.
//
// `data-legato-param` / `data-mode-param` name the descriptor names this
// cell pairs with; both are resolved per layer at bind time so a layer
// rebind (0045) rebuilds the cell with the new ids.
export function makeDetuneLegato(el, ids, descs, modeName, layer) {
  const { detune, legato, mode } = ids;
  const label = el.dataset.label || descs.detune.label;
  el.classList.add('ctl-detune');
  el.innerHTML =
    '<div class="ctl-label">' + label.toUpperCase() + '</div>' +
    '<div class="ctl-detune-body">' +
      '<div class="ctl-fader">' +
        '<div class="ctl-fader-track"></div>' +
        '<div class="ctl-fader-thumb"></div>' +
      '</div>' +
      '<div class="ctl-detune-legato ctl-tg-row"></div>' +
    '</div>';
  const fader = el.querySelector('.ctl-fader');
  const thumb = el.querySelector('.ctl-fader-thumb');
  const legatoRow = el.querySelector('.ctl-detune-legato');
  tgRow('LEGATO', { mount: legatoRow });

  const DESC_TOP = descs.detune.max;
  // Twin's variant index lives in the assign descriptor (current order:
  // Poly, Unison, Solo, Twin → index 3). Look it up by name so a reorder
  // in ASSIGN_LABELS doesn't desync.
  const lookupVariant = (name) => variantIdx(modeName, name, layer);
  const TWIN_IDX = lookupVariant('Twin');
  const MONO_IDXS = new Set();
  // Mono assign modes (Legato applies in these): Unison, Solo. Found by
  // name so an ASSIGN_LABELS reorder doesn't desync.
  ['Unison', 'Solo'].forEach((n) => {
    const i = lookupVariant(n);
    if (i >= 0) MONO_IDXS.add(i);
  });

  let lastDetunePlain = 0;
  let lastModePlain = 0;

  function currentTop() {
    return Math.round(lastModePlain) === TWIN_IDX ? TWIN_TOP_CT : DESC_TOP;
  }
  function setThumbFromPlain(plain) {
    const top = currentTop();
    paintFader(fader, thumb, top > 0 ? plain / top : 0);
  }

  let drag;
  let lastDetuneDisplay = null;
  const detuneLabel = () =>
    lastDetuneDisplay || (lastDetunePlain.toFixed(1) + ' ct');
  const pop = attachValuePop({
    isHovered:  () => drag.isHovered(),
    isDragging: () => drag.isDragging(),
  }, detuneLabel);
  drag = wireFaderDrag(fader, {
    onEnter: (ev) => pop.markEntered(ev),
    onLeave: () => pop.markLeft(),
    onDown: (ev, n) => {
      window.vxn.send.beginGesture(detune);
      const plain = n * currentTop();
      lastDetunePlain = plain;
      lastDetuneDisplay = plain.toFixed(1) + ' ct';
      setThumbFromPlain(plain);
      window.vxn.send.setParam(detune, plain);
      pop.markGrabbed(ev);
    },
    onMove: (_ev, n) => {
      const plain = n * currentTop();
      lastDetunePlain = plain;
      lastDetuneDisplay = plain.toFixed(1) + ' ct';
      setThumbFromPlain(plain);
      window.vxn.send.setParam(detune, plain);
      pop.refresh();
    },
    onUp: () => {
      window.vxn.send.endGesture(detune);
      pop.markReleased();
    },
  });

  legatoRow.addEventListener('pointerdown', (ev) => {
    ev.preventDefault();
    const on = legatoRow.classList.contains('active') ? 0 : 1;
    window.vxn.send.discrete(legato, on);
  });
  // Double-click resets the detune fader (descriptor default).
  el.addEventListener('dblclick', (ev) => {
    ev.preventDefault();
    window.vxn.send.discrete(detune, descs.detune.default);
  });

  function applyLegatoDim() {
    legatoRow.classList.toggle('disabled', !MONO_IDXS.has(Math.round(lastModePlain)));
  }

  return {
    // The composite registers three model.controls entries (detune, legato, mode)
    // pointing at three updater closures returned here — `init()` then
    // routes each ParamChanged into the matching closure.
    detuneUpdate(plain, _norm, display) {
      lastDetunePlain = plain;
      lastDetuneDisplay = display || (plain.toFixed(1) + ' ct');
      setThumbFromPlain(plain);
      pop.refresh();
    },
    legatoUpdate(plain) {
      legatoRow.classList.toggle('active', plain >= 0.5);
    },
    modeUpdate(plain) {
      const prevTwin = Math.round(lastModePlain) === TWIN_IDX;
      lastModePlain = plain;
      // On entering Twin, clamp the stored detune down to the Twin ceiling
      // (mirrors `vxn_ui_vizia::clamp_detune_on_twin`). The engine doesn't
      // enforce this — Twin's "useful" range is purely a view convention,
      // so the editor that surfaces the mode is the one that has to clamp.
      if (!prevTwin && Math.round(plain) === TWIN_IDX && lastDetunePlain > TWIN_TOP_CT) {
        window.vxn.send.discrete(detune, TWIN_TOP_CT);
        lastDetunePlain = TWIN_TOP_CT;
      }
      setThumbFromPlain(lastDetunePlain);
      applyLegatoDim();
    },
  };
}
