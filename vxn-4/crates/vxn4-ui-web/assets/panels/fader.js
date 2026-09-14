// Vertical fader — the faceplate's default continuous control.
//
// Height is `--fader-h`, which rows re-declare: the travel a fader wants is a
// property of the row it sits in, and a track sized for the densest row reads
// as a hairline in the roomiest one.
//
// Returns the labelled cell, with `cell.api` = { get, set, spec, onChange }.
// `onChange` exists for the views that mirror a fader elsewhere — the EG graph
// draws what its eight faders say, and the operator header prints the ratio
// four tuning faders come out to.

import { formatValue } from './value.js';
import { attachPop } from './pop.js';
import { dragNorm } from './drag.js';

export function fader(label, spec, t0) {
  const cell = document.createElement("div");
  cell.className = "ctl";
  const lbl = document.createElement("div");
  lbl.className = "ctl-label";
  lbl.textContent = label;
  const track = document.createElement("div");
  track.className = "ctl-fader";
  track.innerHTML =
    '<div class="ctl-fader-track"><div class="ctl-fader-fill"></div></div>' +
    '<div class="ctl-fader-thumb"></div>';
  cell.append(lbl, track);

  const home = t0 ?? 0.5;
  let t = home;
  let onChange = null;
  const fill = track.querySelector(".ctl-fader-fill");
  const thumb = track.querySelector(".ctl-fader-thumb");
  function draw() {
    fill.style.height = (t * 100) + "%";
    thumb.style.bottom = (t * 100) + "%";
    if (onChange) onChange(t);
  }
  attachPop(track, () => label + "  " + formatValue(spec, t));
  dragNorm(track, {
    get: () => t,
    set: (nt) => { t = nt; draw(); },
    axis: "y",
    span: 120,
    onDoubleClick: () => { t = home; draw(); },
  });
  draw();
  cell.api = {
    get: () => t,
    set: (nt) => { t = nt; draw(); },
    spec,
    onChange: (fn) => { onChange = fn; },
  };
  return cell;
}
