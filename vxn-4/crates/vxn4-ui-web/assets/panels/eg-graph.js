// The selected operator's amplitude EG, with draggable breakpoints.
//
// The four-rate / four-level DX shape the engine already runs
// (`vxn4_engine::eg`), one per operator per voice — not the same thing as
// PERFORM's two ADHSR mod sources.
//
// The graph and the eight faders are two views of one set of numbers: each
// handle carries both axes, horizontal being the segment's time and vertical
// the level it arrives at, and dragging writes straight back into the faders
// so the two views never disagree. vxn-2's `panels/eg-graph.js` is the same
// idiom (it draws; it does not drag), which is why this is a new module rather
// than an import of that one.

import { fitCanvas } from './canvas.js';

export function createEgGraph(canvas, model) {
  // `{pad, gw, gh, total, handles}` from the last paint. Hit-testing reads it
  // rather than recomputing: the handle positions are already the answer, and
  // two copies of the layout maths would be two chances to disagree.
  let geom = null;
  let drag = null;

  function draw() {
    const t = model.times();
    const l = model.levels();
    if (t.length < 4 || l.length < 4) return;
    const { ctx, w, h } = fitCanvas(canvas);
    ctx.clearRect(0, 0, w, h);

    const pad = 12;
    const gw = w - pad * 2, gh = h - pad * 2 - 8;
    ctx.strokeStyle = "#16241d";
    ctx.lineWidth = 1;
    for (let i = 0; i <= 4; i++) {
      const y = pad + (gh * i) / 4;
      ctx.beginPath(); ctx.moveTo(pad, y); ctx.lineTo(w - pad, y); ctx.stroke();
    }

    // The sustain span is drawn, not stored: it is however long the key is
    // held. Scaled off the attack-to-decay total so the picture stays inside
    // the box whatever the times are.
    const sustain = (t[0] + t[1] + t[2]) * 0.45 + 0.2;
    const total = t[0] + t[1] + t[2] + sustain + t[3];
    const x = (secs) => pad + (secs / total) * gw;
    const y = (lev) => pad + gh * (1 - lev);

    const tA = t[0], tB = tA + t[1], tC = tB + t[2], tD = tC + sustain;
    const pts = [[0, 0], [tA, l[0]], [tB, l[1]], [tC, l[2]], [tD, l[2]], [total, l[3]]];

    ctx.beginPath();
    ctx.moveTo(x(pts[0][0]), y(pts[0][1]));
    pts.slice(1).forEach((p) => ctx.lineTo(x(p[0]), y(p[1])));
    ctx.strokeStyle = "#d9701b";
    ctx.lineWidth = 1.75;
    ctx.stroke();

    ctx.lineTo(x(total), y(0));
    ctx.lineTo(x(0), y(0));
    ctx.closePath();
    ctx.fillStyle = "rgba(217, 112, 27, 0.12)";
    ctx.fill();

    // Sustain span, marked so the flat section is not read as another segment.
    ctx.setLineDash([3, 3]);
    ctx.strokeStyle = "#55707e";
    ctx.lineWidth = 1;
    [tC, tD].forEach((s) => {
      ctx.beginPath(); ctx.moveTo(x(s), pad); ctx.lineTo(x(s), pad + gh); ctx.stroke();
    });
    ctx.setLineDash([]);

    // Handles: the four the player can move. The sustain corner is not one —
    // its position is a consequence of D2 and L3, not a parameter.
    const handles = [
      { i: 0, px: x(tA), py: y(l[0]) },
      { i: 1, px: x(tB), py: y(l[1]) },
      { i: 2, px: x(tC), py: y(l[2]) },
      { i: 3, px: x(total), py: y(l[3]) },
    ];
    handles.forEach((hd) => {
      ctx.beginPath();
      ctx.arc(hd.px, hd.py, 4, 0, Math.PI * 2);
      ctx.fillStyle = drag === hd.i ? "#fff" : "#87afb2";
      ctx.fill();
      ctx.strokeStyle = "#a7cfe2";
      ctx.lineWidth = 1;
      ctx.stroke();
    });

    ctx.fillStyle = "#6f7780";
    ctx.font = "9px system-ui";
    ctx.fillText("A", x(tA / 2) - 3, h - 2);
    ctx.fillText("D1", x(tA + t[1] / 2) - 5, h - 2);
    ctx.fillText("D2", x(tB + t[2] / 2) - 5, h - 2);
    ctx.fillText("SUS", x(tC + sustain / 2) - 8, h - 2);
    ctx.fillText("R", x(total - t[3] / 2) - 3, h - 2);

    geom = { pad, gw, gh, total, handles };
  }

  canvas.addEventListener("pointerdown", (e) => {
    if (!geom) return;
    const r = canvas.getBoundingClientRect();
    const mx = e.clientX - r.left, my = e.clientY - r.top;
    let best = null, bestD = 12 * 12;
    geom.handles.forEach((hd) => {
      const d = (hd.px - mx) ** 2 + (hd.py - my) ** 2;
      if (d < bestD) { bestD = d; best = hd; }
    });
    if (!best) return;
    drag = best.i;
    try { canvas.setPointerCapture(e.pointerId); } catch (_) { /* no capture: the drag still tracks */ }
    draw();
  });

  canvas.addEventListener("pointermove", (e) => {
    if (drag === null || !geom) return;
    const r = canvas.getBoundingClientRect();
    const mx = e.clientX - r.left, my = e.clientY - r.top;
    const { pad, gw, gh, total } = geom;

    // Vertical: straight to the level fader.
    model.setLevel(drag, Math.min(1, Math.max(0, 1 - (my - pad) / gh)));

    // Horizontal: the handle sits at the END of its segment, so the segment's
    // own time is the gap between this handle and the previous one.
    const t = model.times();
    const starts = [0, t[0], t[0] + t[1], null];
    const secs = ((mx - pad) / gw) * total;
    if (drag < 3) {
      model.setTime(drag, Math.max(0.001, secs - starts[drag]));
    } else {
      // Release runs from the end of the sustain span to the right edge.
      model.setTime(3, Math.max(0.001, secs - (total - t[3])));
    }
    draw();
  });

  ["pointerup", "pointercancel"].forEach((ev) => canvas.addEventListener(ev, (e) => {
    if (drag === null) return;
    drag = null;
    try { canvas.releasePointerCapture(e.pointerId); } catch (_) { /* already released */ }
    draw();
  }));

  return { draw };
}
