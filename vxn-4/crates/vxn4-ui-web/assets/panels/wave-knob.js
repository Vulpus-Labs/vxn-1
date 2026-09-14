// Rotary waveform selector — the LFO shape picker.
//
// A port of vxn-2's `panels/knob.js` (itself a port of vxn-1's), reshaped from
// "bind to a param on existing markup" into "build the control" because that
// is how this faceplate is assembled. The glyph table and the arc placement
// are vxn-2's verbatim, so the two instruments' LFOs read identically.
//
// It is a third copy of that glyph table and it should not stay one: lifting
// `knob.js` into `vxn-core-ui-web/assets/` is E008's job and would mean editing
// vxn-2, which ticket 0387 is scoped out of. Flagged here so the next E008
// ticket finds it.
//
// Click a glyph to select. There is no popup: the lit glyph IS the readout,
// and an enum has nothing a formatted value would add.

import { ARC_START, ARC_SWEEP } from './dial.js';

export const WAVE_GLYPHS = {
  "Sine": (() => {
    const pts = [];
    for (let k = 0; k <= 16; k++) { const u = k / 16; pts.push([u, 0.5 - 0.38 * Math.sin(u * Math.PI * 2)]); }
    return pts;
  })(),
  "Tri":   [[0, 0.85], [0.5, 0.15], [1, 0.85]],
  "Saw+":  [[0, 0.85], [0.5, 0.15], [0.5, 0.85], [1, 0.15]],
  "Saw-":  [[0, 0.15], [0.5, 0.85], [0.5, 0.15], [1, 0.85]],
  "Pulse": [[0, 0.85], [0, 0.15], [0.5, 0.15], [0.5, 0.85], [1, 0.85]],
  "S&H":   [[0, 0.6], [0.28, 0.6], [0.28, 0.2], [0.56, 0.2], [0.56, 0.8], [0.82, 0.8], [0.82, 0.45], [1, 0.45]],
  "Sqr":   [[0, 0.85], [0, 0.15], [0.5, 0.15], [0.5, 0.85], [1, 0.85]],
};

export const LFO_SHAPES = ["Sine", "Tri", "Saw+", "Saw-", "Pulse", "S&H"];

export function glyphPath(name, w, h) {
  const pts = WAVE_GLYPHS[name];
  if (!pts) return null;
  return pts.map((p, i) => (i === 0 ? "M" : "L") + (p[0] * w).toFixed(2) + " " + (p[1] * h).toFixed(2)).join(" ");
}

export function waveKnob(label, variants, idx0, size) {
  size = size || 64;
  const cell = document.createElement("div");
  cell.className = "ctl";
  const lbl = document.createElement("div");
  lbl.className = "ctl-label";
  lbl.textContent = label;
  const holder = document.createElement("div");
  holder.className = "ctl-wave";
  cell.append(lbl, holder);

  let idx = idx0 || 0;
  const cx = size / 2, cy = size / 2;
  const knobR = size * 0.20, glyphR = size * 0.41, gw = size * 0.22, gh = size * 0.16;
  const n = variants.length;
  const stepDeg = n > 1 ? ARC_SWEEP / (n - 1) : 0;

  function draw() {
    let svg = `<svg width="${size}" height="${size}" viewBox="0 0 ${size} ${size}">`;
    svg += `<circle class="knob-face" cx="${cx}" cy="${cy}" r="${knobR.toFixed(2)}"/>`;
    svg += `<circle class="knob-dimple" cx="${cx}" cy="${cy}" r="${(knobR * 0.62).toFixed(2)}"/>`;
    for (let i = 0; i < n; i++) {
      const a = (ARC_START + i * stepDeg) * Math.PI / 180;
      const gx = cx + glyphR * Math.sin(a), gy = cy - glyphR * Math.cos(a);
      const d = glyphPath(variants[i], gw, gh);
      svg += `<g transform="translate(${(gx - gw / 2).toFixed(2)} ${(gy - gh / 2).toFixed(2)})" data-variant="${i}">`;
      // A transparent rect behind each glyph: a 1.2px stroke is not a hit
      // target anyone can land on.
      svg += `<rect class="wave-hit" x="-3" y="-3" width="${(gw + 6).toFixed(2)}" height="${(gh + 6).toFixed(2)}"/>`;
      if (d) svg += `<path class="wave-glyph${i === idx ? " active" : ""}" d="${d}"/>`;
      svg += `</g>`;
    }
    const ang = (ARC_START + idx * stepDeg).toFixed(2);
    svg += `<g transform="rotate(${ang} ${cx} ${cy})">`;
    svg += `<line class="knob-indicator-line" x1="${cx}" y1="${cy}" x2="${cx}" y2="${(cy - knobR + 2).toFixed(2)}"/>`;
    svg += `</g></svg>`;
    holder.innerHTML = svg;
  }
  holder.addEventListener("pointerdown", (e) => {
    const g = e.target.closest("[data-variant]");
    if (g) { idx = +g.dataset.variant; draw(); }
  });
  draw();
  cell.api = { get: () => idx, set: (i) => { idx = i; draw(); } };
  return cell;
}
