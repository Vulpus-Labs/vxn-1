import { describe, it, expect, beforeEach } from 'vitest';
import {
  METER_FLOOR_DB, METER_CLIP_DB, METER_DECAY_DB_PER_S, METER_HOLD_MS,
  METER_PEAK_DECAY_DB_PER_S,
  GR_RELEASE_DB_PER_S,
  toDb, dbToNorm, advanceMeter, initialMeterState, grToNorm, advanceGr, makeMeter,
} from '../panels.js';

beforeEach(() => {
  document.body.innerHTML = '';
});

describe('toDb', () => {
  it('maps unity to 0 dBFS', () => {
    expect(toDb(1)).toBeCloseTo(0, 6);
  });

  it('maps half amplitude to about −6 dB', () => {
    expect(toDb(0.5)).toBeCloseTo(-6.0206, 3);
  });

  it('clamps silence to the floor rather than −Infinity', () => {
    // −Infinity would poison every downstream sum; the floor keeps the
    // ballistics arithmetic finite.
    expect(toDb(0)).toBe(METER_FLOOR_DB);
    expect(Number.isFinite(toDb(0))).toBe(true);
  });

  it('clamps anything below the floor to the floor', () => {
    expect(toDb(1e-9)).toBe(METER_FLOOR_DB);
  });

  it('is robust to negative / non-numeric input', () => {
    expect(toDb(-0.5)).toBe(METER_FLOOR_DB);
    expect(toDb(NaN)).toBe(METER_FLOOR_DB);
  });
});

describe('dbToNorm', () => {
  it('puts the floor at 0 and full scale at 1', () => {
    expect(dbToNorm(METER_FLOOR_DB)).toBe(0);
    expect(dbToNorm(0)).toBe(1);
  });

  it('is linear in dB — equal screen distance is equal ratio', () => {
    // Halfway up the bar is halfway down the dB range.
    expect(dbToNorm(METER_FLOOR_DB / 2)).toBeCloseTo(0.5, 6);
  });

  it('clamps out-of-range input', () => {
    expect(dbToNorm(12)).toBe(1);
    expect(dbToNorm(-200)).toBe(0);
  });
});

describe('advanceMeter — attack', () => {
  it('jumps to a new peak instantly', () => {
    // A meter must never under-report a transient, so attack is not smoothed.
    const s = advanceMeter(initialMeterState(), 1.0, 16);
    expect(s.db).toBeCloseTo(0, 6);
  });

  it('rises fully even when the frame interval is tiny', () => {
    const s = advanceMeter(initialMeterState(), 0.5, 0.1);
    expect(s.db).toBeCloseTo(toDb(0.5), 6);
  });
});

describe('advanceMeter — decay', () => {
  it('falls at the documented dB-per-second rate', () => {
    let s = advanceMeter(initialMeterState(), 1.0, 16);   // pinned at 0 dB
    s = advanceMeter(s, 0, 1000);                          // one second of silence
    expect(s.db).toBeCloseTo(-METER_DECAY_DB_PER_S, 6);
  });

  it('accumulates the same fall across many small steps as one big one', () => {
    let many = advanceMeter(initialMeterState(), 1.0, 16);
    for (let i = 0; i < 100; i++) many = advanceMeter(many, 0, 10);
    let one = advanceMeter(initialMeterState(), 1.0, 16);
    one = advanceMeter(one, 0, 1000);
    expect(many.db).toBeCloseTo(one.db, 6);
  });

  it('never falls below the incoming level', () => {
    // Decaying past a signal that is still present would under-report it.
    let s = advanceMeter(initialMeterState(), 1.0, 16);
    s = advanceMeter(s, 0.5, 1000);
    expect(s.db).toBeCloseTo(toDb(0.5), 6);
  });

  it('bottoms out at the floor, not below', () => {
    let s = advanceMeter(initialMeterState(), 1.0, 16);
    for (let i = 0; i < 50; i++) s = advanceMeter(s, 0, 100);
    expect(s.db).toBe(METER_FLOOR_DB);
  });
});

describe('advanceMeter — peak hold', () => {
  it('latches the highest level seen', () => {
    let s = advanceMeter(initialMeterState(), 1.0, 16);
    s = advanceMeter(s, 0.1, 16);
    expect(s.peakDb).toBeCloseTo(0, 6);
  });

  it('holds the marker for the hold time before it starts falling', () => {
    let s = advanceMeter(initialMeterState(), 1.0, 16);
    // Just short of the hold expiring: still pinned.
    s = advanceMeter(s, 0, METER_HOLD_MS - 32);
    expect(s.peakDb).toBeCloseTo(0, 6);
  });

  it('falls at the peak rate once the hold expires', () => {
    let s = advanceMeter(initialMeterState(), 1.0, 16);
    s = advanceMeter(s, 0, METER_HOLD_MS);   // burns the hold down to 0
    const held = s.peakDb;
    s = advanceMeter(s, 0, 100);             // now it falls
    expect(held).toBeCloseTo(0, 6);
    expect(s.peakDb).toBeCloseTo(-METER_PEAK_DECAY_DB_PER_S * 0.1, 6);
  });

  it('restarts the hold when a new high arrives', () => {
    let s = advanceMeter(initialMeterState(), 0.5, 16);
    s = advanceMeter(s, 0, METER_HOLD_MS);   // hold exhausted
    expect(s.holdMs).toBe(0);
    s = advanceMeter(s, 1.0, 16);            // new high
    expect(s.holdMs).toBe(METER_HOLD_MS);
    expect(s.peakDb).toBeCloseTo(0, 6);
  });

  it('never lets the marker sit below the bar', () => {
    // The marker falls faster than the bar, so without the clamp it could
    // dip under a sustained level and read as the bar overtaking its peak.
    let s = advanceMeter(initialMeterState(), 1.0, 16);
    s = advanceMeter(s, 0, METER_HOLD_MS);
    for (let i = 0; i < 30; i++) s = advanceMeter(s, 0.25, 50);
    expect(s.peakDb).toBeGreaterThanOrEqual(s.db);
  });
});

describe('grToNorm', () => {
  it('reads zero for no reduction', () => {
    expect(grToNorm(0)).toBe(0);
    // Positive is makeup gain, not reduction.
    expect(grToNorm(4)).toBe(0);
  });

  it('scales reduction against the range', () => {
    expect(grToNorm(-10, 20)).toBeCloseTo(0.5, 6);
    expect(grToNorm(-20, 20)).toBeCloseTo(1, 6);
  });

  it('clamps past full scale', () => {
    expect(grToNorm(-40, 20)).toBe(1);
  });
});

describe('makeMeter — DOM', () => {
  it('builds one bar per channel', () => {
    const el = document.createElement('div');
    makeMeter(el, { channels: 2 });
    expect(el.querySelectorAll('.meter-bar').length).toBe(2);
    expect(el.classList.contains('meter-level')).toBe(true);
  });

  it('paints fill height from the pushed peak', () => {
    const el = document.createElement('div');
    const m = makeMeter(el, { channels: 2 });
    m.push([1.0, 0]);
    m.render(16);
    const fills = el.querySelectorAll('.meter-fill');
    expect(fills[0].style.height).toBe('100%');
    expect(fills[1].style.height).toBe('0%');
  });

  it('folds two pushes between renders by taking the louder', () => {
    // Mirrors the Rust-side bus: an intermediate frame must not erase a
    // louder one that arrived first.
    const el = document.createElement('div');
    const m = makeMeter(el, { channels: 1 });
    m.push([1.0]);
    m.push([0.001]);
    m.render(16);
    expect(el.querySelector('.meter-fill').style.height).toBe('100%');
  });

  it('decays when frames stop carrying signal', () => {
    const el = document.createElement('div');
    const m = makeMeter(el, { channels: 1 });
    m.push([1.0]);
    m.render(16);
    const loud = parseFloat(el.querySelector('.meter-fill').style.height);
    // A render with nothing pushed: the frame is consumed, so this really is
    // silence rather than a stale value being reused.
    m.render(500);
    const quiet = parseFloat(el.querySelector('.meter-fill').style.height);
    expect(quiet).toBeLessThan(loud);
  });

  it('flags clipping at full scale and not below', () => {
    const el = document.createElement('div');
    const m = makeMeter(el, { channels: 1 });
    m.push([1.0]);
    m.render(16);
    expect(el.querySelector('.meter-bar').classList.contains('clipped')).toBe(true);

    const el2 = document.createElement('div');
    const m2 = makeMeter(el2, { channels: 1 });
    m2.push([0.5]);
    m2.render(16);
    expect(el2.querySelector('.meter-bar').classList.contains('clipped')).toBe(false);
    expect(toDb(0.5)).toBeLessThan(METER_CLIP_DB);
  });

  it('reset returns every bar to rest', () => {
    const el = document.createElement('div');
    const m = makeMeter(el, { channels: 2 });
    m.push([1.0, 1.0]);
    m.render(16);
    m.reset();
    for (const f of el.querySelectorAll('.meter-fill')) {
      expect(f.style.height).toBe('0%');
    }
    expect(el.querySelector('.meter-bar').classList.contains('clipped')).toBe(false);
  });

  it('gr meters draw a single channel with no peak marker', () => {
    const el = document.createElement('div');
    const m = makeMeter(el, { channels: 1, kind: 'gr', grRange: 20 });
    expect(el.classList.contains('meter-gr')).toBe(true);
    // The wire carries reduction as NEGATIVE dB, which is what the engine
    // publishes — pushing a positive magnitude here would not exercise the
    // real path.
    m.push([-10]);
    m.render(16);
    expect(el.querySelector('.meter-fill').style.height).toBe('50%');
    expect(el.querySelector('.meter-peak').style.display).toBe('none');
  });

  it('gr meters read empty for no reduction', () => {
    const el = document.createElement('div');
    const m = makeMeter(el, { channels: 1, kind: 'gr', grRange: 20 });
    m.push([0]);
    m.render(16);
    expect(el.querySelector('.meter-fill').style.height).toBe('0%');
  });

  it('gr meters keep the deepest reduction between renders', () => {
    const el = document.createElement('div');
    const m = makeMeter(el, { channels: 1, kind: 'gr', grRange: 20 });
    m.push([-4]);
    m.push([-16]);
    m.push([-2]);
    m.render(16);
    expect(el.querySelector('.meter-fill').style.height).toBe('80%');
  });

  it('level meters still ignore negative input', () => {
    // Only the gr path treats sign as magnitude; a level is a linear peak and
    // a negative one is meaningless.
    const el = document.createElement('div');
    const m = makeMeter(el, { channels: 1 });
    m.push([-0.5]);
    m.render(16);
    expect(el.querySelector('.meter-fill').style.height).toBe('0%');
  });
});

describe('GR ballistics', () => {
  it('rises instantly to a deeper reduction', () => {
    // Under-reporting how hard the compressor grabbed would be the one
    // unacceptable error, so attack is not smoothed.
    expect(advanceGr(0, 12, 16)).toBe(12);
    expect(advanceGr(3, 12, 16)).toBe(12);
  });

  it('releases at the documented dB-per-second rate', () => {
    // 100 ms of release, short enough that the floor clamp doesn't bite.
    expect(advanceGr(20, 0, 100)).toBeCloseTo(20 - GR_RELEASE_DB_PER_S * 0.1, 6);
  });

  it('does not release past the current reduction', () => {
    expect(advanceGr(10, 8, 1000)).toBe(8);
  });

  it('bottoms out at zero, never negative', () => {
    expect(advanceGr(5, 0, 10_000)).toBe(0);
  });

  it('holds through a render that carries no frame — the flicker fix', () => {
    // Meter frames arrive on the editor tick; renders run on rAF. A render
    // landing between two frames drains 0, and without the hold the bar would
    // collapse to empty and back, which is what the flicker was.
    let mag = advanceGr(0, 15, 16);
    expect(mag).toBe(15);
    mag = advanceGr(mag, 0, 16);              // gap render, no frame
    expect(mag).toBeGreaterThan(14);          // barely moved, not zeroed
  });
});

describe('makeMeter — gr bar does not flicker across frameless renders', () => {
  it('keeps most of its height when a render carries no frame', () => {
    const el = document.createElement('div');
    const m = makeMeter(el, { channels: 1, kind: 'gr', grRange: 20 });
    m.push([-20]);
    m.render(16);
    expect(el.querySelector('.meter-fill').style.height).toBe('100%');
    // Next rAF tick with no meter frame in between.
    m.render(16);
    const h = parseFloat(el.querySelector('.meter-fill').style.height);
    expect(h).toBeGreaterThan(90);
  });
});
