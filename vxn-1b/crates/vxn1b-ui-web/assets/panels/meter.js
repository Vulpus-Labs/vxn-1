// panels/meter.js — level meters (ticket 0240).
//
// The audio side publishes raw peak magnitudes into a lock-free bus and the
// controller drains one `{kind:'meters'}` frame per ~60 Hz tick. Everything
// visual happens here: dB mapping, ballistics, peak-hold, clip flag. Keeping
// it view-side means the curve can be tuned without touching DSP, and holds
// the audio thread to one atomic per channel per block.
//
// A frame's value is the **peak since the previous frame**, so a tick that
// reports 0 genuinely means "silence since you last looked" — the decay below
// can trust it rather than having to guess whether an update was simply missed.
//
// ESM `import`/`export` lines are dropped by the splice loader (bindings ride
// the concatenated scope); under Node they resolve, so the vitest suites can
// pull the pure helpers directly.

// Meter scale floor. Below this a bar reads empty. −60 dB is the usual mixer
// choice: low enough that a fading tail stays visible, high enough that the
// top 20 dB — where mixing decisions actually happen — get most of the length.
export const METER_FLOOR_DB = -60;

// Anything at or above this reads as clipped. Not 0.0 exactly: inter-sample
// peaks and the reconstruction filter can push a converter over even when no
// sample hits full scale, so the warning wants a little headroom.
export const METER_CLIP_DB = -0.1;

// Fall rate of the bar, dB per second. 20 dB/s is the broadcast-meter
// convention — fast enough to track a phrase, slow enough that the eye can
// read a transient instead of seeing it flicker.
export const METER_DECAY_DB_PER_S = 20;

// How long the peak marker sits before it starts falling, in ms.
export const METER_HOLD_MS = 1000;

// The marker falls faster than the bar once the hold expires, so it rejoins a
// sustained level rather than shadowing it indefinitely.
export const METER_PEAK_DECAY_DB_PER_S = 40;

// Linear magnitude → dBFS. Zero (and anything below the floor) clamps to the
// floor rather than −Infinity, so downstream arithmetic stays finite.
export function toDb(linear) {
  if (!(linear > 0)) return METER_FLOOR_DB;
  const db = 20 * Math.log10(linear);
  return db < METER_FLOOR_DB ? METER_FLOOR_DB : db;
}

// dBFS → [0,1] bar fill. Linear in dB, which is what makes a meter readable:
// equal screen distance is equal ratio.
export function dbToNorm(db) {
  const n = (db - METER_FLOOR_DB) / (0 - METER_FLOOR_DB);
  return n < 0 ? 0 : n > 1 ? 1 : n;
}

// Advance one channel's ballistics by `dtMs`, given the newly-arrived peak.
//
// Pure function of (state, input, elapsed) — no clock read, no DOM — so the
// suite can drive it deterministically and the render path can call it from a
// rAF loop. Returns a NEW state object; the caller swaps it in.
//
//   state = { db, peakDb, holdMs }
//
// Attack is instantaneous (a meter must never under-report a transient);
// release is the linear dB-per-second fall above. The peak marker latches the
// highest value seen, holds for METER_HOLD_MS, then falls on its own slope.
export function advanceMeter(state, peakLinear, dtMs) {
  const inDb = toDb(peakLinear);
  const dt = Math.max(0, dtMs) / 1000;

  // Bar: jump up instantly, ease down at the decay rate.
  let db = state.db;
  if (inDb >= db) {
    db = inDb;
  } else {
    db = Math.max(inDb, db - METER_DECAY_DB_PER_S * dt);
  }

  // Peak marker: re-latch on a new high and restart the hold; otherwise burn
  // down the hold first, and only then fall.
  let peakDb = state.peakDb;
  let holdMs = state.holdMs;
  if (inDb >= peakDb) {
    peakDb = inDb;
    holdMs = METER_HOLD_MS;
  } else if (holdMs > 0) {
    holdMs = Math.max(0, holdMs - dtMs);
  } else {
    peakDb = Math.max(db, peakDb - METER_PEAK_DECAY_DB_PER_S * dt);
  }

  // The marker never sits below the bar — that would read as the bar having
  // overtaken its own peak.
  if (peakDb < db) peakDb = db;

  return { db, peakDb, holdMs };
}

// A channel at rest.
export function initialMeterState() {
  return { db: METER_FLOOR_DB, peakDb: METER_FLOOR_DB, holdMs: 0 };
}

// Gain reduction (dB, ≤ 0) → [0,1] downward fill. Full scale at
// `-rangeDb`, so a compressor pinned at −20 dB shows a full bar.
export function grToNorm(db, rangeDb = 20) {
  if (!(db < 0)) return 0;
  const n = -db / rangeDb;
  return n > 1 ? 1 : n;
}

// Release rate of the GR bar, dB per second.
//
// The reduction reading needs ballistics for the same reason a level does, and
// the reason is mechanical rather than aesthetic: meter frames arrive on the
// editor's ~60 Hz tick while the bar renders on rAF, and the two are not in
// lockstep. Whenever a render falls between two frames the drained value is 0,
// so a bar driven straight off the frame collapses to empty and back every few
// renders — visible as flicker. Holding the reading and releasing it smoothly
// makes the display independent of that phase relationship.
//
// Faster than the level decay: gain reduction genuinely does recover quickly,
// and a slow release would misreport how hard the compressor is working.
export const GR_RELEASE_DB_PER_S = 60;

// Advance the GR bar. `grDbMag` is the reduction depth as a POSITIVE magnitude
// (the frame carries it negative; the widget takes `abs` on the way in).
// Instant attack so a transient grab is never under-reported, timed release.
export function advanceGr(prevMag, grDbMag, dtMs) {
  const target = grDbMag > 0 ? grDbMag : 0;
  if (target >= prevMag) return target;
  const released = prevMag - GR_RELEASE_DB_PER_S * (Math.max(0, dtMs) / 1000);
  return released > target ? released : target;
}

// ─── DOM widget ────────────────────────────────────────────────────────────
//
// Builds a stereo (or N-channel) meter into `el` and returns a handle with
// `push(values)` — called once per frame with the raw linear peaks — and
// `render(dtMs)`, called from the animation loop. Splitting push from render
// means a dropped or late frame decays smoothly instead of freezing the bar.

export function makeMeter(el, { channels = 2, kind = 'level', grRange = 20 } = {}) {
  el.classList.add('meter', kind === 'gr' ? 'meter-gr' : 'meter-level');
  el.innerHTML = '';
  const bars = [];
  for (let i = 0; i < channels; i++) {
    const bar = document.createElement('div');
    bar.className = 'meter-bar';
    const fill = document.createElement('div');
    fill.className = 'meter-fill';
    const peak = document.createElement('div');
    peak.className = 'meter-peak';
    bar.appendChild(fill);
    bar.appendChild(peak);
    el.appendChild(bar);
    // `grMag` is the GR bar's held reduction depth (positive dB). Levels use
    // `state`; the two paths never share a channel.
    bars.push({ bar, fill, peak, state: initialMeterState(), grMag: 0 });
  }

  // Latest pushed frame. Held rather than rendered immediately so the render
  // loop owns timing — see the push/render split above.
  let pending = new Array(channels).fill(0);

  return {
    push(values) {
      for (let i = 0; i < channels; i++) {
        const v = values[i];
        if (typeof v !== 'number' || !isFinite(v)) continue;
        // Gain reduction arrives as NEGATIVE dB (0 = none, −12 = 12 dB down);
        // levels arrive as positive linear magnitudes. Take the magnitude so
        // both fold the same way — without this a reduction frame would clamp
        // to 0 and the GR bar would never move.
        const mag = kind === 'gr' ? Math.abs(v) : v > 0 ? v : 0;
        // Fold rather than overwrite: if two frames land between renders, the
        // larger must win, exactly as the Rust-side bus does.
        pending[i] = Math.max(pending[i], mag);
      }
    },
    render(dtMs) {
      for (let i = 0; i < channels; i++) {
        const ch = bars[i];
        if (kind === 'gr') {
          // Held + released rather than taken straight from the frame: renders
          // and meter frames are not in lockstep, so a render landing between
          // two frames sees 0 and the bar would flicker empty.
          ch.grMag = advanceGr(ch.grMag, pending[i], dtMs);
          ch.fill.style.height = (grToNorm(-ch.grMag, grRange) * 100) + '%';
          ch.peak.style.display = 'none';
        } else {
          ch.state = advanceMeter(ch.state, pending[i], dtMs);
          ch.fill.style.height = (dbToNorm(ch.state.db) * 100) + '%';
          ch.peak.style.bottom = (dbToNorm(ch.state.peakDb) * 100) + '%';
          ch.peak.style.display = ch.state.peakDb > METER_FLOOR_DB ? '' : 'none';
          ch.bar.classList.toggle('clipped', ch.state.peakDb >= METER_CLIP_DB);
        }
      }
      // Consume the frame. Anything arriving before the next render folds in
      // from zero again, so a silent interval really does decay.
      pending = new Array(channels).fill(0);
    },
    reset() {
      pending = new Array(channels).fill(0);
      for (const ch of bars) {
        ch.state = initialMeterState();
        ch.grMag = 0;
        ch.fill.style.height = '0%';
        ch.peak.style.display = 'none';
        ch.bar.classList.remove('clipped');
      }
    },
  };
}

// ─── Registry + animation loop ─────────────────────────────────────────────
//
// Meters are addressed by frame key ('master', 'l1', 'l2', 'dynIn', 'dynGr') so
// dispatch can fan one frame out without knowing which panels exist. A single
// rAF loop drives every registered meter — one loop, not one per widget.

export const meterRegistry = (() => {
  const meters = new Map();
  let raf = null;
  let last = null;

  function tick(now) {
    const dt = last == null ? 16 : now - last;
    last = now;
    for (const m of meters.values()) m.render(dt);
    raf = requestAnimationFrame(tick);
  }

  return {
    register(key, meter) {
      meters.set(key, meter);
      // Start on first registration, not at module load — a faceplate with no
      // meters (or a headless test) never spins the loop.
      if (raf == null && typeof requestAnimationFrame === 'function') {
        raf = requestAnimationFrame(tick);
      }
    },
    // Fan a decoded meter frame out to whichever meters are mounted. Unknown
    // keys are ignored, so the Rust side can ship taps the faceplate has not
    // wired a panel for yet (dynamics lands in 0241).
    apply(frame) {
      for (const [key, meter] of meters) {
        const v = frame[key];
        if (v == null) continue;
        meter.push(Array.isArray(v) ? v : [v]);
      }
    },
    reset() {
      for (const m of meters.values()) m.reset();
    },
    // One mounted meter by key. The scope panel's meter is registered under a
    // key no frame carries (`scope`) so `apply` skips it — it shows whichever
    // layer is being edited, which is a view decision, so the dispatcher feeds
    // it by hand from the frame's `l1` / `l2` while the rAF loop here still
    // drives its ballistics.
    get(key) {
      return meters.get(key) || null;
    },
    // Test seam: the suites drive `render` directly rather than via rAF.
    _meters: meters,
  };
})();
