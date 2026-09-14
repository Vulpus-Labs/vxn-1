// Horizontal bipolar fader — a route's authored constant, and a matrix slot's
// depth.
//
// Horizontal because it lives in a table row, where the only dimension going
// spare is width. Bipolar by default because a PM depth's sign is not
// cosmetic: a modulator inverted against its neighbour cancels where it would
// otherwise reinforce. The centre tick and the fill-from-centre are what make
// "no depth" look like nothing rather than like half of something.

import { formatValue } from './value.js';
import { attachPop } from './pop.js';
import { dragNorm } from './drag.js';

export function hFader(t0, spec, label, onChange) {
  const el = document.createElement("div");
  el.className = "hfader";
  el.innerHTML =
    '<div class="hfader-track"><div class="hfader-fill"></div></div>' +
    '<div class="hfader-centre"></div>' +
    '<div class="hfader-thumb"></div>';
  const fill = el.querySelector(".hfader-fill");
  const thumb = el.querySelector(".hfader-thumb");
  const centre = el.querySelector(".hfader-centre");
  if (!spec.bipolar) centre.style.display = "none";

  let t = t0;
  function draw() {
    const origin = spec.bipolar ? 0.5 : 0;
    const lo = Math.min(origin, t), hi = Math.max(origin, t);
    fill.style.left = (lo * 100) + "%";
    fill.style.width = ((hi - lo) * 100) + "%";
    thumb.style.left = (t * 100) + "%";
    if (onChange) onChange(t);
  }
  attachPop(el, () => label + "  " + formatValue(spec, t));
  dragNorm(el, {
    get: () => t,
    set: (nt) => { t = nt; draw(); },
    axis: "x",
    span: 160,
    // Home is the spec's own neutral, not the value the row happened to load
    // with: on a bipolar depth that is zero, and zero is the thing a
    // double-click is reaching for.
    onDoubleClick: () => { t = spec.bipolar ? 0.5 : 0; draw(); },
  });
  draw();
  el.api = { get: () => t, set: (nt) => { t = nt; draw(); } };
  return el;
}
