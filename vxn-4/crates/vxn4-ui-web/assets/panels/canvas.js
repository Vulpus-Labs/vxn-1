// Canvas plumbing shared by the three graphs.
//
// The canvases are laid out by flexbox and sized in CSS pixels, so their
// backing store has to be resized to match whatever the layout gave them —
// times the device pixel ratio, or every line is a blurred pair on a retina
// display. This is done per paint rather than on a resize observer: a paint is
// cheap, the canvases are small, and the alternative is a class of bug where
// the graph is crisp until the first tab switch.

export function fitCanvas(c) {
  const r = c.getBoundingClientRect();
  const dpr = window.devicePixelRatio || 1;
  c.width = Math.max(1, Math.round(r.width * dpr));
  c.height = Math.max(1, Math.round(r.height * dpr));
  const ctx = c.getContext("2d");
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  return { ctx, w: r.width, h: r.height };
}

/** The background rule grid, drawn the same way in all three graphs. */
export function graphGrid(ctx, w, h, cols, rows) {
  ctx.strokeStyle = "#16241d";
  ctx.lineWidth = 1;
  for (let i = 1; i < cols; i++) {
    const x = (w * i) / cols;
    ctx.beginPath(); ctx.moveTo(x, 0); ctx.lineTo(x, h); ctx.stroke();
  }
  for (let i = 1; i < rows; i++) {
    const y = (h * i) / rows;
    ctx.beginPath(); ctx.moveTo(0, y); ctx.lineTo(w, y); ctx.stroke();
  }
}
