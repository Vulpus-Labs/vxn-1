// The shared mod-matrix curve control (0340) — glyph rendering, the 3×3
// picker's open → pick → close cycle, and the three cancel paths.
//
// Imported from `vxn-core-ui-web/assets/` directly, like the other shared-
// primitive suites (`wire-drag`, `cutoff-tuned`): the module is spliced into
// both faceplates, so it is tested once rather than through either panel.
import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  createCurveButton,
  curveGlyphSvg,
  closeCurvePicker,
  curvePickerElement,
} from '../../../../../crates/vxn-core-ui-web/assets/curve-picker.js';

// Nine stub glyphs. The real ones come from `vxn_core_matrix::glyph`, which is
// where the geometry is tested; what matters here is that the control draws the
// points it is handed and no others.
const GLYPHS = Array.from({ length: 9 }, (_, code) => ({
  code,
  points: `0,${code} 100,${100 - code}`,
  band_x: code === 3 || code === 4 || code === 5 ? 50 : 0,
  band_w: code === 3 || code === 4 || code === 5 ? 50 : 100,
}));
const LABELS = [
  'Lin', 'Exp', 'Log',
  'Bipolar', 'Bipolar Exp', 'Bipolar Log',
  'Abs', 'Abs Exp', 'Abs Log',
];
// None / Abs / Bipolar down, Lin / Exp / Log across.
const CODES = [0, 1, 2, 6, 7, 8, 3, 4, 5];

let picked;

function button(code = 0, extra = {}) {
  const btn = createCurveButton({
    glyphs: GLYPHS,
    labels: LABELS,
    codes: CODES,
    title: 'Curve',
    code,
    onPick: (...a) => picked.push(a),
    ...extra,
  });
  document.body.appendChild(btn);
  return btn;
}

const click = (el) => el.dispatchEvent(new MouseEvent('click', { bubbles: true }));
const picker = () => document.querySelector('.cg-picker');
const isOpen = () => !!picker() && !picker().hidden;
const opt = (code) => picker().querySelector(`.cg-opt[data-code="${code}"]`);

beforeEach(() => {
  closeCurvePicker();
  document.body.innerHTML = '';
  picked = [];
});

describe('curveGlyphSvg', () => {
  it('draws the points it is given, and nothing it is not', () => {
    const svg = curveGlyphSvg(GLYPHS[4], false);
    expect(svg).toContain('points="0,4 100,96"');
    // Row scale: a faint zero line only. No axis cross, no identity, no band —
    // at 38×22 all nine read as hash marks with the frame in.
    expect(svg).toContain('cg-faint');
    expect(svg).not.toContain('cg-ident');
    expect(svg).not.toContain('cg-band');
  });

  it('adds the frame at picker scale, with the band the polarity expects', () => {
    const svg = curveGlyphSvg(GLYPHS[4], true);
    expect(svg).toContain('cg-ident');
    // `Bipolar` is written for a unipolar source, so its band is the right half.
    expect(svg).toContain('class="cg-band" x="50"');
    expect(svg).toContain('width="50"');
    expect(svg).not.toContain('cg-faint');
  });

  it('is empty for a code with no glyph rather than throwing', () => {
    expect(curveGlyphSvg(undefined, false)).toBe('');
  });
});

describe('the curve button', () => {
  it('draws its current curve and names it', () => {
    const btn = button(7);
    expect(btn.innerHTML).toContain('points="0,7 100,93"');
    expect(btn.title).toBe('Curve — Abs Exp');
    expect(btn.dataset.code).toBe('7');
    expect(btn.code()).toBe(7);
  });

  it('repaints on setCode', () => {
    const btn = button(0);
    btn.setCode(5);
    expect(btn.innerHTML).toContain('points="0,5 100,95"');
    expect(btn.title).toBe('Curve — Bipolar Log');
    expect(btn.code()).toBe(5);
  });

  it('is a real button, never a native select', () => {
    // An NSMenu steals webview first-responder under macOS/WKWebView, which
    // swallows the first click after one closes. Both panels hand-roll their
    // pick-lists for this reason and the picker must not undo it.
    button();
    expect(document.querySelectorAll('select')).toHaveLength(0);
    expect(document.querySelector('.cg-btn').tagName).toBe('BUTTON');
  });
});

describe('the picker', () => {
  it('opens anchored to the button, with the current curve marked', () => {
    const btn = button(4);
    click(btn);
    expect(isOpen()).toBe(true);
    expect(btn.classList.contains('cg-open')).toBe(true);
    expect(btn.getAttribute('aria-expanded')).toBe('true');
    expect(opt(4).classList.contains('cg-sel')).toBe(true);
    expect(opt(0).classList.contains('cg-sel')).toBe(false);
  });

  it('lays the grid out None / Abs / Bipolar down, Lin / Exp / Log across', () => {
    click(button());
    const codes = [...picker().querySelectorAll('.cg-opt')].map((o) => +o.dataset.code);
    expect(codes).toEqual(CODES);
    const heads = [...picker().querySelectorAll('.cg-rh')].map((h) => h.textContent);
    expect(heads).toEqual(['None', 'Abs', 'Bipolar']);
    const cols = [...picker().querySelectorAll('.cg-ch')].map((h) => h.textContent);
    expect(cols).toEqual(['Lin', 'Exp', 'Log']);
  });

  it('picking applies, closes, and reports the pair already split', () => {
    const btn = button(0);
    click(btn);
    click(opt(7));
    // `(code, polarity, shape)` — the panels differ on which they send, so the
    // control hands over all three rather than choosing for them.
    expect(picked).toEqual([[7, 2, 1]]);
    expect(isOpen()).toBe(false);
    expect(btn.code()).toBe(7);
    expect(btn.classList.contains('cg-open')).toBe(false);
  });

  it('clicking the open button again closes it without editing', () => {
    const btn = button(0);
    click(btn);
    click(btn);
    expect(isOpen()).toBe(false);
    expect(picked).toEqual([]);
  });

  it('only one is open at a time', () => {
    const a = button(0);
    const b = button(3);
    click(a);
    click(b);
    expect(isOpen()).toBe(true);
    expect(a.classList.contains('cg-open')).toBe(false);
    expect(b.classList.contains('cg-open')).toBe(true);
    expect(opt(3).classList.contains('cg-sel')).toBe(true);
  });
});

describe('the cancel paths', () => {
  it('Esc cancels without editing', () => {
    const btn = button(2);
    click(btn);
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    expect(isOpen()).toBe(false);
    expect(picked).toEqual([]);
    expect(btn.code()).toBe(2);
  });

  it('an outside click cancels without editing', () => {
    const btn = button(2);
    const elsewhere = document.createElement('div');
    document.body.appendChild(elsewhere);
    click(btn);
    click(elsewhere);
    expect(isOpen()).toBe(false);
    expect(picked).toEqual([]);
    expect(btn.code()).toBe(2);
  });

  it('a window resize cancels without editing', () => {
    const btn = button(2);
    click(btn);
    window.dispatchEvent(new Event('resize'));
    expect(isOpen()).toBe(false);
    expect(picked).toEqual([]);
    expect(btn.code()).toBe(2);
  });

  it('a scroll under the picker cancels — it is fixed, the button is not', () => {
    const btn = button(2);
    click(btn);
    document.dispatchEvent(new Event('scroll', { bubbles: true }));
    expect(isOpen()).toBe(false);
    expect(picked).toEqual([]);
  });

  it('a cancelled picker leaves no listeners behind to fire on the next one', () => {
    const btn = button(0);
    click(btn);
    document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
    // A second cycle must still work — a stale `onPick` or a doubled listener
    // would show up here as either no edit or two.
    click(btn);
    click(opt(8));
    expect(picked).toEqual([[8, 2, 2]]);
    expect(isOpen()).toBe(false);
  });
});

describe('the singleton', () => {
  it('is not created until a picker first opens', () => {
    // Importing the module must not touch the DOM — same contract `value-pop`
    // has, so a page that only splices it pays nothing.
    expect(document.querySelector('.cg-picker')).toBeNull();
    click(button());
    expect(curvePickerElement()).toBeTruthy();
  });

  it('re-attaches after the page replaces body', () => {
    click(button());
    document.body.innerHTML = '';
    const btn = button(1);
    click(btn);
    expect(isOpen()).toBe(true);
    expect(picker().isConnected).toBe(true);
  });
});

describe('closeCurvePicker', () => {
  it('closes an open picker for a panel hiding its overlay', () => {
    const btn = button(0);
    click(btn);
    closeCurvePicker();
    expect(isOpen()).toBe(false);
    expect(btn.classList.contains('cg-open')).toBe(false);
    expect(picked).toEqual([]);
  });

  it('is a no-op when nothing is open', () => {
    expect(() => closeCurvePicker()).not.toThrow();
  });
});

// `vi` is imported for parity with the sibling suites; the control needs no
// mocks, which is itself worth noticing — it takes a callback, not a bridge.
void vi;
