// panels/scope.js — the layer oscilloscope.
//
// The audio side keeps a lock-free ring of the selected layer's output and the
// controller ships one window per ~30 Hz tick as `{kind:'scope', s:[…]}`. Every
// display decision lives here: the trigger search, the clipping, the drawing.
// Same division of labour as the meters — the engine publishes raw samples and
// the view decides what they look like — so the trace can be retuned without
// touching DSP.
//
// Drawing happens on frame arrival rather than in a rAF loop. A meter needs its
// own clock because it has ballistics that must keep decaying between frames; a
// scope has no state of its own — a frame IS the picture — so a render loop
// would just redraw identical pixels 60 times a second.
//
// ESM `import`/`export` lines are dropped by the splice loader (bindings ride
// the concatenated scope); under Node they resolve, so the vitest suites can
// pull the pure helpers directly.

// Fraction of the incoming window reserved for the trigger search. The trace
// starts wherever the trigger lands inside it, so the drawn span is the
// remainder — 3/4 of the frame, whether a crossing was found or not (a
// not-found frame starts at the search limit), which keeps the time base
// constant instead of breathing with the signal.
export const SCOPE_SEARCH_FRACTION = 0.25;

// Vertical range of the display, in sample units. ±1 is full scale, so a
// layer pushed a little past unity still shows its shape rather than a pair of
// flat clipped rails.
export const SCOPE_RANGE = 1.25;

// Index of the first rising zero-crossing at or before `searchEnd`, or null if
// the window has none.
//
// Triggering is the difference between a waveform and a smear: consecutive
// frames start at unrelated phases, so an untriggered trace of a steady note
// slides sideways at whatever the beat frequency between the note and the frame
// rate happens to be. Anchoring each frame to the same feature of the waveform
// holds it still.
//
// Rising specifically (`<= 0` then `> 0`), not any crossing: picking whichever
// came first would alternate between the two edges of a symmetric wave and
// flip the trace upside down every other frame.
export function findFirstRisingCross(samples, searchEnd) {
  const end = Math.min(searchEnd, samples.length - 1);
  for (let i = 1; i <= end; i++) {
    if (samples[i - 1] <= 0 && samples[i] > 0) return i;
  }
  return null;
}

// Where the drawn span starts in `samples`, given the search budget. Returns
// the trigger index when one was found, else the search limit — so an
// untriggered frame (silence, or a wave with no rising crossing in the search
// window) still draws the same number of samples.
export function scopeStart(samples, searchFraction = SCOPE_SEARCH_FRACTION) {
  const searchEnd = Math.floor(samples.length * searchFraction);
  if (searchEnd < 1) return 0;
  const cross = findFirstRisingCross(samples, searchEnd);
  return cross == null ? searchEnd : cross;
}

// ─── DOM widget ────────────────────────────────────────────────────────────
//
// Builds a canvas into `el` and returns a handle with `push(samples)` (called
// on each arriving frame) and `clear()` (called when the tap changes, so a
// stale layer's trace never lingers on screen while the new ring refills).

export function makeScope(el) {
  el.classList.add('scope');
  el.innerHTML = '';
  const canvas = document.createElement('canvas');
  canvas.className = 'scope-canvas';
  el.appendChild(canvas);

  let samples = null;

  // The canvas backing store is sized in device pixels against the CSS box.
  // Re-measured on every draw: the editor is fixed-size, but the layer pane is
  // `display: none` while another tab is up, and a canvas measured then would
  // be 0×0 and stay blank after the tab came back.
  function resize() {
    const dpr = window.devicePixelRatio || 1;
    const w = Math.round(el.clientWidth * dpr);
    const h = Math.round(el.clientHeight * dpr);
    if (w > 0 && h > 0 && (canvas.width !== w || canvas.height !== h)) {
      canvas.width = w;
      canvas.height = h;
    }
    return canvas.width > 0 && canvas.height > 0;
  }

  function draw() {
    if (!resize()) return;
    const ctx = canvas.getContext && canvas.getContext('2d');
    if (!ctx) return;
    const w = canvas.width;
    const h = canvas.height;
    const dpr = window.devicePixelRatio || 1;

    ctx.clearRect(0, 0, w, h);
    ctx.fillStyle = '#0e0e0e';
    ctx.fillRect(0, 0, w, h);

    const yFor = (v) => ((1 - v / SCOPE_RANGE) / 2) * (h - 1);

    // Centre line solid, the ±1 rails dimmer: the rails are a scale reference,
    // the centre is where a silent layer sits and wants to be readable.
    ctx.lineWidth = dpr;
    for (const [rail, colour] of [[-1, '#2a2a2a'], [1, '#2a2a2a'], [0, '#3a3a3a']]) {
      ctx.strokeStyle = colour;
      ctx.beginPath();
      const y = Math.round(yFor(rail)) + 0.5;
      ctx.moveTo(0, y);
      ctx.lineTo(w, y);
      ctx.stroke();
    }

    if (!samples || samples.length < 2) return;
    const start = scopeStart(samples);
    const n = samples.length - start;
    if (n < 2) return;

    ctx.strokeStyle = '#6cb1ff';
    ctx.lineWidth = 1.5 * dpr;
    ctx.lineJoin = 'round';
    ctx.beginPath();
    const stride = (w - 1) / (n - 1);
    for (let j = 0; j < n; j++) {
      const v = Math.max(-SCOPE_RANGE, Math.min(SCOPE_RANGE, samples[start + j]));
      const x = j * stride;
      const y = yFor(v);
      if (j === 0) ctx.moveTo(x, y);
      else ctx.lineTo(x, y);
    }
    ctx.stroke();
  }

  // First paint: the rails, so the panel reads as an instrument rather than a
  // hole in the faceplate before any audio arrives.
  draw();

  return {
    push(next) {
      samples = Array.isArray(next) ? next : null;
      draw();
    },
    clear() {
      samples = null;
      draw();
    },
    // Test seam.
    _canvas: canvas,
  };
}
