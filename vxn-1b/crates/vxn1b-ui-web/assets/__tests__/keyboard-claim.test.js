// The shared keyboard claim (0364) — the page's half of the deal that keeps
// the DAW's spacebar, computer-MIDI keyboard and shortcuts working while the
// editor is open.
//
// Imported from `vxn-core-ui-web/assets/` directly, like the other shared-
// primitive suites (`curve-picker`, `wire-drag`, `cutoff-tuned`): the module is
// spliced into every faceplate, so it is tested once rather than through one.
//
// What matters here is the wire traffic — the shell's focus guard reads exactly
// one boolean — so every case asserts on the posted messages.
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import {
  createKeyboardClaim,
  installKeyboardClaim,
  isTextEntry,
  KEYBOARD_CLAIM_OP,
  TEXT_ENTRY_TOKEN,
} from '../../../../../crates/vxn-core-ui-web/assets/keyboard-claim.js';

let posted;
let claim;
// Deferred focus-out re-checks run through this so a test can drain them
// deterministically instead of awaiting a real microtask.
let pending;

function drain() {
  const q = pending;
  pending = [];
  for (const fn of q) fn();
}

function make(extra) {
  return createKeyboardClaim(
    Object.assign(
      {
        post: (msg) => posted.push(msg),
        document,
        window,
        defer: (fn) => pending.push(fn),
      },
      extra || {},
    ),
  );
}

/// The claim only sends transitions, so "what the shell now believes" is the
/// last `on` posted — `null` if it was never told anything.
function state() {
  return posted.length ? posted[posted.length - 1].on : null;
}

beforeEach(() => {
  posted = [];
  pending = [];
  document.body.innerHTML = '';
  delete window.__vxnKeyboard;
});

afterEach(() => {
  if (claim) claim.dispose();
  claim = null;
});

describe('isTextEntry', () => {
  it('accepts the places a user types', () => {
    const text = document.createElement('input');
    text.type = 'text';
    const bare = document.createElement('input'); // no type → text
    const area = document.createElement('textarea');
    const rich = document.createElement('div');
    // jsdom doesn't implement `isContentEditable` off the attribute.
    Object.defineProperty(rich, 'isContentEditable', { value: true });
    for (const el of [text, bare, area, rich]) expect(isTextEntry(el)).toBe(true);
  });

  it('rejects controls that carry no typed text', () => {
    // A fader taking focus must NOT take the keyboard off the host — this is
    // the case that would silently re-break transport on every knob click.
    for (const type of ['range', 'checkbox', 'radio', 'button', 'color']) {
      const el = document.createElement('input');
      el.type = type;
      expect(isTextEntry(el), type).toBe(false);
    }
    expect(isTextEntry(document.createElement('button'))).toBe(false);
    expect(isTextEntry(document.body)).toBe(false);
    expect(isTextEntry(null)).toBe(false);
  });
});

describe('explicit claims', () => {
  it('sends one message per transition, not per call', () => {
    claim = make();
    claim.claim('a');
    claim.claim('a');
    expect(posted).toEqual([{ op: KEYBOARD_CLAIM_OP, on: true }]);
    claim.release('a');
    expect(posted).toEqual([
      { op: KEYBOARD_CLAIM_OP, on: true },
      { op: KEYBOARD_CLAIM_OP, on: false },
    ]);
  });

  it('holds the keyboard until the last token is released', () => {
    // A curve picker opened over the preset panel must not hand the keyboard
    // back when it alone closes.
    claim = make();
    claim.claim('preset-browser');
    claim.claim('curve-picker');
    claim.release('curve-picker');
    expect(state()).toBe(true);
    expect(claim.tokens()).toEqual(['preset-browser']);
    claim.release('preset-browser');
    expect(state()).toBe(false);
  });

  it('sends its first state unconditionally, then only transitions', () => {
    // A page reload builds a fresh tracker but leaves the shell's flag where
    // the old page left it — stuck true if a claim was held at reload. So the
    // first sync always posts, even when it is a no-op release, and only
    // repeats after that are suppressed.
    claim = make();
    claim.release('never-claimed');
    expect(posted).toEqual([{ op: KEYBOARD_CLAIM_OP, on: false }]);
    claim.release('also-never-held');
    expect(posted).toHaveLength(1);
  });
});

describe('automatic text-entry claim', () => {
  function focusIn(el) {
    el.dispatchEvent(new FocusEvent('focusin', { bubbles: true, target: el }));
  }
  function focusOut(el) {
    el.dispatchEvent(new FocusEvent('focusout', { bubbles: true }));
  }

  it('claims on focus into a text field and releases on the way out', () => {
    const input = document.createElement('input');
    input.type = 'text';
    document.body.appendChild(input);
    claim = make();

    input.focus();
    focusIn(input);
    expect(state()).toBe(true);

    input.blur();
    focusOut(input);
    // Not released yet — the re-check is deferred on purpose.
    expect(state()).toBe(true);
    drain();
    expect(state()).toBe(false);
  });

  it('does not claim for a fader', () => {
    const fader = document.createElement('input');
    fader.type = 'range';
    document.body.appendChild(fader);
    claim = make();
    focusIn(fader);
    expect(posted).toEqual([]);
  });

  it('holds the claim while focus moves between two fields', () => {
    // `focusout` fires before the next `focusin`. Releasing eagerly would drop
    // the claim for a beat — long enough for the shell's ~60 Hz tick to yank
    // focus out of a half-typed preset name.
    const a = document.createElement('input');
    const b = document.createElement('input');
    document.body.append(a, b);
    claim = make();

    a.focus();
    focusIn(a);
    expect(state()).toBe(true);
    posted.length = 0;

    focusOut(a);
    b.focus();
    focusIn(b);
    drain();

    expect(posted, 'no release should have been sent').toEqual([]);
    expect(claim.held).toBe(true);
  });

  it('drops the text claim when the host window takes focus back', () => {
    const input = document.createElement('input');
    document.body.appendChild(input);
    claim = make();
    focusIn(input);
    expect(state()).toBe(true);

    window.dispatchEvent(new Event('blur'));
    expect(state()).toBe(false);
  });

  it('keeps an overlay claim across a window blur', () => {
    // The overlay is still open; the host having focus is not a reason to
    // forget that. Only the text claim is speculative enough to drop.
    claim = make();
    claim.claim('matrix-overlay');
    focusIn(document.body); // not text entry, no-op
    window.dispatchEvent(new Event('blur'));
    expect(state()).toBe(true);
    expect(claim.tokens()).toEqual(['matrix-overlay']);
  });
});

describe('install', () => {
  it('is idempotent so a second splice cannot stack listeners', () => {
    const first = installKeyboardClaim({ post: () => {}, document, window });
    const second = installKeyboardClaim({ post: () => {}, document, window });
    expect(second).toBe(first);
    expect(window.__vxnKeyboard).toBe(first);
    first.dispose();
  });

  it('does not auto-install without wry ipc present', () => {
    // Importing the module in Node must not install anything — the suite
    // injects its own transport. `window.ipc` is absent under jsdom.
    expect(window.ipc).toBeUndefined();
    expect(window.__vxnKeyboard).toBeUndefined();
  });
});

describe('token cleanup', () => {
  it('dispose stops listening and forgets its claims', () => {
    const input = document.createElement('input');
    document.body.appendChild(input);
    const c = make();
    c.claim('x');
    c.dispose();
    posted.length = 0;
    focusInto(input);
    expect(posted).toEqual([]);
    expect(c.held).toBe(false);
  });

  function focusInto(el) {
    el.dispatchEvent(new FocusEvent('focusin', { bubbles: true }));
  }
});
