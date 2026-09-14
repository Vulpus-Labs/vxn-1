// Operator waveform picker — a dropdown with a drawn preview above it.
//
// vxn-4's own; there is no precedent to port. Eleven waveforms is past what a
// rotary selector can carry (the glyphs collide on the arc and the hit targets
// stop being clickable), and unlike the LFO's shape an operator's waveform is
// a configuration choice rather than a performed one. The LFO keeps its knob.
//
// The preview is drawn from the IDEAL shape, sampled. The engine's tables are
// band-limited by summing `harmonic_amp`, so a preview drawn from the series
// would show Gibbs ringing — true of the table, and useless as a picture of
// which waveform this is.

import { combo } from './combo.js';

const TAU = Math.PI * 2;

// A Dirichlet kernel — the sum of the first n cosine harmonics at equal
// amplitude, normalised. Stands in for the impulse table, which is what a
// finite harmonic stack actually sounds like.
const dirichlet = (th, n) => {
  let s = 0;
  for (let k = 1; k <= n; k++) s += Math.cos(k * th);
  return s / n;
};

/* Operator waveforms.
 *
 * The first four are `vxn4_dsp::Waveform` as it stands. The rest are the
 * proposed additions, each one arm of `harmonic_amp` plus one table set. They
 * are drawn here and greyed by 0388 until the engine has them — a greyed
 * control says "planned, not yet"; an absent one says "never".
 *
 * The sine family is the substantive part: every waveform in the current set
 * is symmetric, so every one produces odd-ordered sideband structure. The
 * rectified and gated variants carry EVEN harmonics and waveform asymmetry,
 * which is what a sine cannot reach at any ratio or index — clav, reed and
 * vocal-ish FM live there.
 *
 * `dc` flags a non-zero mean. On a modulator that is only a constant phase
 * offset; on a carrier it is bias at the sum bus — a note-on thump and lost
 * headroom — so it is a property worth seeing before you pick, not after.
 */
export const OP_WAVE_DEFS = [
  { name: "Sine", dc: false, f: (th) => Math.sin(th) },
  { name: "Triangle", dc: false, f: (th) => 1 - 4 * Math.abs(((th / TAU) + 0.25) % 1 - 0.5) },
  { name: "Saw", dc: false, f: (th) => 1 - th / Math.PI },
  { name: "Square", dc: false, f: (th) => (th < Math.PI ? 1 : -1) },
  { name: "Half Sine", dc: true, f: (th) => (th < Math.PI ? Math.sin(th) : 0) },
  { name: "Abs Sine", dc: true, f: (th) => Math.abs(Math.sin(th)) },
  { name: "Quarter Sine", dc: true, f: (th) => (th < Math.PI / 2 ? Math.sin(2 * th) : 0) },
  { name: "Pinch Sine", dc: true, f: (th) => (th < Math.PI ? Math.abs(Math.sin(2 * th)) : 0) },
  { name: "Pulse 25%", dc: true, f: (th) => (th < Math.PI / 2 ? 1 : -1) },
  { name: "Pulse 12%", dc: true, f: (th) => (th < Math.PI / 4 ? 1 : -1) },
  { name: "Impulse", dc: false, f: (th) => dirichlet(th, 12) },
];

/** One cycle of `def`, as an inline SVG string. */
export function wavePreviewSvg(def, w, h) {
  const mid = h / 2, amp = h / 2 - 3;
  let d = "";
  const N = 96;
  for (let i = 0; i <= N; i++) {
    const th = (i / N) * TAU;
    // Clamped a little past full scale: the point is the shape, and a kernel
    // with a tall centre lobe would otherwise squash everything else flat.
    const v = Math.max(-1.15, Math.min(1.15, def.f(th)));
    d += (i === 0 ? "M" : "L") + ((i / N) * (w - 4) + 2).toFixed(2) + " " + (mid - v * amp).toFixed(2);
  }
  return `<svg class="wave-preview" width="${w}" height="${h}" viewBox="0 0 ${w} ${h}">` +
         `<line class="wave-axis" x1="0" y1="${mid}" x2="${w}" y2="${mid}"/>` +
         `<path class="wave-trace" d="${d}"/></svg>`;
}

export function wavePicker(label, defs, idx0, onChange) {
  const cell = document.createElement("div");
  cell.className = "ctl";
  const lbl = document.createElement("div");
  lbl.className = "ctl-label";
  lbl.textContent = label;
  const pick = document.createElement("div");
  pick.className = "wave-pick";
  const preview = document.createElement("div");
  preview.className = "wave-preview-holder";
  const chip = document.createElement("div");
  chip.className = "dc-chip";
  chip.textContent = "DC";
  chip.title = "Non-zero mean — biases the sum bus when used as a carrier";
  const sel = combo(defs.map((d) => d.name), idx0);
  pick.append(preview, sel, chip);
  cell.append(lbl, pick);

  let idx = idx0 || 0;
  function draw() {
    preview.innerHTML = wavePreviewSvg(defs[idx], 92, 34);
    // `visibility`, not `display`: the chip keeps its space so picking a
    // DC-free waveform does not move the dropdown under the pointer.
    chip.classList.toggle("hidden", !defs[idx].dc);
  }
  sel.addEventListener("change", () => { idx = +sel.value; draw(); if (onChange) onChange(idx); });
  draw();
  cell.api = { get: () => idx, set: (i) => { idx = i; sel.value = String(i); draw(); } };
  return cell;
}
