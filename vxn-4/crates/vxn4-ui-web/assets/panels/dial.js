// Rotary dial, and the arc geometry the wave knob shares with it.
//
// A dial rather than a fader wherever the panel has width but no height — the
// macro row, the dynamics grid, the key-scaling side column. Same drag, same
// popup; the difference is what the control costs in layout.
//
// `ARC_START` / `ARC_SWEEP` are the 270° sweep from the 7-o'clock position
// that vxn-1b and vxn-2 both use. They live here rather than in the wave knob
// because the dial is the primitive the knob's indicator geometry follows.

import { formatValue } from './value.js';
import { attachPop } from './pop.js';
import { dragNorm } from './drag.js';

export const ARC_START = -135;
export const ARC_SWEEP = 270;

export function polar(cx, cy, r, deg) {
  const a = (deg - 90) * Math.PI / 180;
  return [cx + r * Math.cos(a), cy + r * Math.sin(a)];
}

export function arcPath(cx, cy, r, a0, a1) {
  const [x0, y0] = polar(cx, cy, r, a0);
  const [x1, y1] = polar(cx, cy, r, a1);
  const large = Math.abs(a1 - a0) > 180 ? 1 : 0;
  return `M${x0.toFixed(2)} ${y0.toFixed(2)} A${r} ${r} 0 ${large} 1 ${x1.toFixed(2)} ${y1.toFixed(2)}`;
}

export function dial(label, spec, t0, size) {
  size = size || 36;
  const cell = document.createElement("div");
  cell.className = "ctl";
  const lbl = document.createElement("div");
  lbl.className = "ctl-label";
  lbl.textContent = label;
  const holder = document.createElement("div");
  holder.className = "ctl-dial";
  cell.append(lbl, holder);

  const home = t0 ?? 0.5;
  let t = home;
  let onChange = null;
  const cx = size / 2, cy = size / 2, r = size * 0.40, faceR = size * 0.29;
  function draw() {
    if (onChange) onChange(t);
    const a1 = ARC_START + ARC_SWEEP * t;
    // Bipolar specs fill outward from centre rather than from the left stop,
    // so "no modulation" reads as an empty arc, not a half-full one.
    const from = spec.bipolar ? ARC_START + ARC_SWEEP * 0.5 : ARC_START;
    const [ix, iy] = polar(cx, cy, faceR - 1, a1);
    holder.innerHTML =
      `<svg width="${size}" height="${size}" viewBox="0 0 ${size} ${size}">` +
      `<path class="dial-track" stroke-width="3" d="${arcPath(cx, cy, r, ARC_START, ARC_START + ARC_SWEEP)}"/>` +
      (Math.abs(a1 - from) > 0.5
        ? `<path class="dial-fill" stroke-width="3" d="${arcPath(cx, cy, r, Math.min(from, a1), Math.max(from, a1))}"/>`
        : "") +
      `<circle class="dial-face" cx="${cx}" cy="${cy}" r="${faceR}"/>` +
      `<line class="dial-indicator" x1="${cx}" y1="${cy}" x2="${ix.toFixed(2)}" y2="${iy.toFixed(2)}"/>` +
      `</svg>`;
  }
  attachPop(holder, () => label + "  " + formatValue(spec, t));
  dragNorm(holder, {
    get: () => t,
    set: (nt) => { t = nt; draw(); },
    axis: "y",
    span: 140,
    onDoubleClick: () => { t = home; draw(); },
  });
  draw();
  cell.api = {
    get: () => t,
    set: (nt) => { t = nt; draw(); },
    // Same seam as the fader's: for the callers that have somewhere to put the
    // value — a per-operator record that has to survive the panel being
    // rebuilt when the selected operator changes.
    onChange: (fn) => { onChange = fn; },
  };
  return cell;
}
