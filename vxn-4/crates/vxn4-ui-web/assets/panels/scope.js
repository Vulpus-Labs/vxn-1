// Output scope.
//
// Draws whatever frame it is handed and nothing when it is handed none. The
// mockup animated a stand-in trace — three detuned saws through a soft
// nonlinearity — which is right for arguing about panel proportions and wrong
// in the plugin: a moving waveform that is not the instrument's waveform is a
// lie told sixty times a second.
//
// So until the engine's scope tap exists (the frame arrives as a `ViewEvent`,
// ticket 0388) this paints the graticule and the zero line, which is what a
// scope with no signal on it looks like.

import { fitCanvas, graphGrid } from './canvas.js';

export function createScope(canvas) {
  /** `samples` is a flat array of -1..1, or null/empty for "no signal". */
  function draw(samples) {
    const { ctx, w, h } = fitCanvas(canvas);
    ctx.clearRect(0, 0, w, h);
    graphGrid(ctx, w, h, 8, 4);
    ctx.strokeStyle = "#2b4a3a";
    ctx.beginPath(); ctx.moveTo(0, h / 2); ctx.lineTo(w, h / 2); ctx.stroke();

    if (!samples || samples.length < 2) return;
    ctx.strokeStyle = "#7fe0a8";
    ctx.lineWidth = 1.5;
    ctx.beginPath();
    for (let px = 0; px <= w; px++) {
      const i = Math.min(samples.length - 1, Math.round((px / w) * (samples.length - 1)));
      const y = h / 2 - samples[i] * h * 0.42;
      px === 0 ? ctx.moveTo(px, y) : ctx.lineTo(px, y);
    }
    ctx.stroke();
  }
  return { draw };
}
