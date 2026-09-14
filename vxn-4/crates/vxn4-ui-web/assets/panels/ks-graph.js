// Key-scaling graph, in the idiom of vxn-2's per-op KS panel.
//
// It answers "what does Key Trk bind to": the operator's OUTPUT LEVEL, scaled
// against the played key, with an independent depth and curve either side of a
// break point. A single key-track number cannot say "a little quieter as you
// go up, a lot quieter as you go down", which is the whole reason FM patches
// want this. EG rate scaling is the second, differently-pivoted mechanism and
// gets its own control rather than being folded in.
//
// Three handles: the break point drags along the key axis, the two end handles
// drag vertically. Dragging an end handle through the centre line flips that
// side between cut and boost, so the sign is the gesture rather than a
// separate switch; the Lin/Exp toggles stay explicit.
//
// vxn-2's `panels/ks-graph.js` is the same picture with a different state
// shape — its own `desc`-bound param ids against vxn-4's plain `ks` record —
// so this is the idiom lifted rather than the file.

import { fitCanvas } from './canvas.js';
import { noteName } from '../../../../../crates/vxn-core-ui-web/assets/cutoff-tuned.js';

/** Full-scale travel of the curve, in dB either side of the break point. */
export const KS_DB_SPAN = 48;

/**
 * The curve itself, as a pure function — gain in dB at MIDI note `key`.
 *
 * `ks` is `{ bp, l, r, lExp, rExp, lBoost, rBoost }`: a break point, and a
 * 0..99 depth with a curve and a direction per side. The 60-semitone
 * normaliser is what makes the depth mean the same thing on both sides of a
 * break point that is not in the middle.
 */
export function ksDb(ks, key) {
  const side = key >= ks.bp ? "r" : "l";
  const depth = (side === "r" ? ks.r : ks.l) / 99;
  const isExp = side === "r" ? ks.rExp : ks.lExp;
  const boost = side === "r" ? ks.rBoost : ks.lBoost;
  const u = Math.min(1, Math.abs(key - ks.bp) / 60);
  const g = isExp ? u * u : u;
  return (boost ? 1 : -1) * depth * KS_DB_SPAN * g;
}

export function createKsGraph(canvas, model) {
  let geom = null, drag = null;

  function draw() {
    const ks = model.ks();
    const { ctx, w, h } = fitCanvas(canvas);
    ctx.clearRect(0, 0, w, h);
    const pad = 10, gw = w - pad * 2, gh = h - pad * 2 - 8;
    const cy = pad + gh / 2;
    const xAt = (m) => pad + (m / 127) * gw;
    const yAt = (db) => cy - (Math.max(-KS_DB_SPAN, Math.min(KS_DB_SPAN, db)) / KS_DB_SPAN) * (gh / 2);

    // Octave rules, so the break point can be read off the graph.
    ctx.strokeStyle = "#16241d";
    ctx.lineWidth = 1;
    for (let oct = 0; oct <= 10; oct++) {
      const x = xAt(oct * 12);
      ctx.beginPath(); ctx.moveTo(x, pad); ctx.lineTo(x, pad + gh); ctx.stroke();
    }
    ctx.strokeStyle = "#2b4a3a";
    ctx.beginPath(); ctx.moveTo(pad, cy); ctx.lineTo(w - pad, cy); ctx.stroke();

    ctx.strokeStyle = "#7fe0a8";
    ctx.lineWidth = 1.6;
    ctx.beginPath();
    for (let m = 0; m <= 127; m++) {
      const x = xAt(m), y = yAt(ksDb(ks, m));
      m === 0 ? ctx.moveTo(x, y) : ctx.lineTo(x, y);
    }
    ctx.stroke();

    const bx = xAt(ks.bp);
    ctx.setLineDash([3, 3]);
    ctx.strokeStyle = "#a7cfe2";
    ctx.lineWidth = 1;
    ctx.beginPath(); ctx.moveTo(bx, pad); ctx.lineTo(bx, pad + gh); ctx.stroke();
    ctx.setLineDash([]);

    const handles = [
      { id: "bp", px: bx, py: cy },
      { id: "l", px: xAt(0) + 3, py: yAt(ksDb(ks, 0)) },
      { id: "r", px: xAt(127) - 3, py: yAt(ksDb(ks, 127)) },
    ];
    handles.forEach((hd) => {
      ctx.beginPath();
      ctx.arc(hd.px, hd.py, 4, 0, Math.PI * 2);
      ctx.fillStyle = drag === hd.id ? "#fff" : "#87afb2";
      ctx.fill();
      ctx.strokeStyle = "#a7cfe2";
      ctx.lineWidth = 1;
      ctx.stroke();
    });

    ctx.fillStyle = "#6f7780";
    ctx.font = "9px system-ui";
    ctx.fillText("C1", xAt(24) - 6, h - 2);
    ctx.fillText(noteName(ks.bp), bx - 8, h - 2);
    ctx.fillText("C7", xAt(96) - 6, h - 2);

    geom = { pad, gw, gh, cy };
    geom.handles = handles;
  }

  canvas.addEventListener("pointerdown", (e) => {
    if (!geom) return;
    const r = canvas.getBoundingClientRect();
    const mx = e.clientX - r.left, my = e.clientY - r.top;
    let best = null, bestD = 14 * 14;
    geom.handles.forEach((hd) => {
      const d = (hd.px - mx) ** 2 + (hd.py - my) ** 2;
      if (d < bestD) { bestD = d; best = hd; }
    });
    if (!best) return;
    drag = best.id;
    try { canvas.setPointerCapture(e.pointerId); } catch (_) { /* no capture: the drag still tracks */ }
    draw();
  });

  canvas.addEventListener("pointermove", (e) => {
    if (!drag || !geom) return;
    const ks = model.ks();
    const r = canvas.getBoundingClientRect();
    const mx = e.clientX - r.left, my = e.clientY - r.top;
    const { pad, gw, gh, cy } = geom;
    if (drag === "bp") {
      ks.bp = Math.max(0, Math.min(127, Math.round(((mx - pad) / gw) * 127)));
    } else {
      // Depth from distance off the centre line; which side of it decides cut
      // vs boost. The curve's own gain at the handle is divided back out, so
      // the handle lands where the pointer is even when the break point is
      // close to that end and the curve has barely got going.
      const db = -((my - cy) / (gh / 2)) * KS_DB_SPAN;
      const depth = Math.min(99, Math.round((Math.abs(db) / KS_DB_SPAN) * 99));
      const isExp = drag === "r" ? ks.rExp : ks.lExp;
      const u = Math.min(1, Math.abs((drag === "r" ? 127 : 0) - ks.bp) / 60);
      const g = isExp ? u * u : u;
      const corrected = g > 0.02 ? Math.min(99, Math.round(depth / g)) : depth;
      if (drag === "r") { ks.r = corrected; ks.rBoost = db >= 0; }
      else { ks.l = corrected; ks.lBoost = db >= 0; }
    }
    draw();
    // Optional: where a bound build reports the edit onward. Nothing beside
    // the graph shows anything a graph drag changes, so the page this shipped
    // with does not pass one.
    if (model.onChange) model.onChange();
  });

  ["pointerup", "pointercancel"].forEach((ev) => canvas.addEventListener(ev, (e) => {
    if (!drag) return;
    drag = null;
    try { canvas.releasePointerCapture(e.pointerId); } catch (_) { /* already released */ }
    draw();
  }));

  return { draw };
}
