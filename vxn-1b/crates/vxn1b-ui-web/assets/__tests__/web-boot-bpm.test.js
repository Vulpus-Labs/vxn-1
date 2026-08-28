// 0320: the web-only tempo control. Until this ticket it lived inside a Rust
// string literal, so the vitest glob never reached it — it was the one piece of
// faceplate JS with no test and no lint, despite being the thing that clamps
// and posts `set_tempo`.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { clampBpm, webChromeRow, mountBpmControl } from '../web-boot-bpm.js';

describe('clampBpm', () => {
  it('passes an in-range tempo through', () => {
    expect(clampBpm(120)).toBe(120);
    expect(clampBpm('90')).toBe(90);
    expect(clampBpm(20)).toBe(20);
    expect(clampBpm(300)).toBe(300);
  });

  it('clamps to the engine range at both ends', () => {
    expect(clampBpm(0)).toBe(20);
    expect(clampBpm(-500)).toBe(20);
    expect(clampBpm(1e6)).toBe(300);
  });

  it('rejects a non-finite tempo rather than clamping it', () => {
    // Not a slider that ran past its end — a value that would divide through
    // every synced rate in the engine. `null` means post nothing.
    for (const bad of ['abc', NaN, Infinity, -Infinity]) {
      expect(clampBpm(bad)).toBeNull();
    }
  });

  it('rejects a blank field instead of reading it as zero', () => {
    // `Number('')` is 0, not NaN, so a bare isFinite check clamped an empty
    // box to 20 — and a number input reads back as '' whenever its contents
    // are invalid. Clearing the box is not a tempo edit (0320).
    for (const blank of ['', '   ', null, undefined]) {
      expect(clampBpm(blank)).toBeNull();
    }
  });

  it('keeps fractional tempos — they are legal, just unusual', () => {
    expect(clampBpm(128.5)).toBe(128.5);
  });
});

describe('webChromeRow', () => {
  beforeEach(() => {
    document.body.innerHTML = '';
  });

  it('creates the shared row when the CPU meter has not mounted yet', () => {
    const row = webChromeRow(document);
    expect(row.id).toBe('vxn-web-chrome');
    expect(row.parentElement).toBe(document.body);
  });

  it('reuses an existing row rather than mounting a second one', () => {
    const first = webChromeRow(document);
    const second = webChromeRow(document);
    expect(second).toBe(first);
    expect(document.querySelectorAll('#vxn-web-chrome')).toHaveLength(1);
  });
});

describe('mountBpmControl', () => {
  let ipc;

  beforeEach(() => {
    document.body.innerHTML = '';
    ipc = { postMessage: vi.fn() };
  });

  const sent = () => ipc.postMessage.mock.calls.map((c) => JSON.parse(c[0]));

  it('seeds the engine on mount, so a synced LFO is right before anyone touches it', () => {
    mountBpmControl(document, ipc, 120);
    expect(sent()).toEqual([{ op: 'set_tempo', bpm: 120 }]);
  });

  it('renders a number input carrying the engine range', () => {
    const { input } = mountBpmControl(document, ipc, 120);
    expect(input.type).toBe('number');
    expect(input.min).toBe('20');
    expect(input.max).toBe('300');
    expect(input.value).toBe('120');
    expect(document.querySelector('.vxn-bpm')).not.toBeNull();
  });

  it('posts the clamped value on change, and writes it back into the field', () => {
    const { input } = mountBpmControl(document, ipc, 120);
    ipc.postMessage.mockClear();
    input.value = '900';
    input.dispatchEvent(new Event('change'));
    expect(sent()).toEqual([{ op: 'set_tempo', bpm: 300 }]);
    // The field must show what was actually sent, not what was typed.
    expect(input.value).toBe('300');
  });

  it('posts nothing when the field is cleared or holds junk', () => {
    const { input } = mountBpmControl(document, ipc, 120);
    for (const bad of ['abc', '']) {
      ipc.postMessage.mockClear();
      // jsdom keeps whatever is assigned; a real number input reports '' for
      // both of these, which is the case that used to slam the tempo to 20.
      input.value = bad;
      input.dispatchEvent(new Event('change'));
      expect(ipc.postMessage, `"${bad}" should post nothing`).not.toHaveBeenCalled();
    }
  });

  it('stops keydown so typing a tempo cannot fire the faceplate shortcuts', () => {
    const { input } = mountBpmControl(document, ipc, 120);
    const onBody = vi.fn();
    document.body.addEventListener('keydown', onBody);
    const ev = new KeyboardEvent('keydown', { key: 'm', bubbles: true, cancelable: true });
    input.dispatchEvent(ev);
    expect(onBody).not.toHaveBeenCalled();
  });

  it('mounts beside the CPU meter whichever is first', () => {
    // CPU meter first.
    const row = webChromeRow(document);
    mountBpmControl(document, ipc, 120);
    expect(document.querySelectorAll('#vxn-web-chrome')).toHaveLength(1);
    expect(row.querySelector('.vxn-bpm')).not.toBeNull();
  });
});
