import { describe, it, expect, beforeEach } from 'vitest';
import { paintFader } from '../panels.js';

// jsdom doesn't compute layout, so the fader/thumb dimensions are stubbed
// directly via Object.defineProperty. The helper only reads
// `fader.clientHeight` and `thumb.offsetHeight`.

const FADER_H = 100;
const THUMB_H = 20;
const HALF_THUMB = THUMB_H / 2;
const TRAVEL = FADER_H - THUMB_H; // 80

function makePair(faderH = FADER_H, thumbH = THUMB_H) {
  const fader = document.createElement('div');
  const thumb = document.createElement('div');
  fader.appendChild(thumb);
  document.body.appendChild(fader);
  Object.defineProperty(fader, 'clientHeight', { value: faderH, configurable: true });
  Object.defineProperty(thumb, 'offsetHeight', { value: thumbH, configurable: true });
  return { fader, thumb };
}

describe('paintFader', () => {
  let fader, thumb;

  beforeEach(() => {
    document.body.innerHTML = '';
    ({ fader, thumb } = makePair());
  });

  it('norm = 0 puts the thumb at the bottom (centre at faderH - halfThumb)', () => {
    paintFader(fader, thumb, 0);
    // thumb.style.top is the thumb's *top edge*; centre = top + halfThumb
    // = (halfThumb + travel) + halfThumb = faderH - halfThumb. So
    // thumb.style.top = halfThumb + travel = 10 + 80 = 90.
    expect(thumb.style.top).toBe(`${HALF_THUMB + TRAVEL}px`);
  });

  it('norm = 1 puts the thumb at the top (centre at halfThumb)', () => {
    paintFader(fader, thumb, 1);
    expect(thumb.style.top).toBe(`${HALF_THUMB}px`);
  });

  it('norm = 0.5 puts the thumb at the midpoint', () => {
    paintFader(fader, thumb, 0.5);
    expect(thumb.style.top).toBe(`${HALF_THUMB + 0.5 * TRAVEL}px`);
  });

  it('clamps norm below 0 to the bottom', () => {
    paintFader(fader, thumb, -0.5);
    expect(thumb.style.top).toBe(`${HALF_THUMB + TRAVEL}px`);
    expect(fader.style.getPropertyValue('--fader-norm')).toBe('0');
  });

  it('clamps norm above 1 to the top', () => {
    paintFader(fader, thumb, 1.5);
    expect(thumb.style.top).toBe(`${HALF_THUMB}px`);
    expect(fader.style.getPropertyValue('--fader-norm')).toBe('1');
  });

  it('sets --fader-norm to the clamped norm', () => {
    paintFader(fader, thumb, 0.25);
    expect(fader.style.getPropertyValue('--fader-norm')).toBe('0.25');
  });
});

// Regression: a fader painted inside a `display: none` container.
//
// jsdom reports 0 for `clientHeight` / `offsetHeight` on hidden elements, which
// is exactly what a real browser does — and what made the FX/Global tab's
// faders show thumbs pinned to full scale over correctly-lit tracks. The fill
// is percentage-driven so it stays right; only the pixel-positioned thumb is
// affected, which is why the two disagreed.
describe('paintFader with no layout', () => {
  function unlaidPair() {
    const fader = document.createElement('div');
    const thumb = document.createElement('div');
    fader.appendChild(thumb);
    document.body.appendChild(fader);
    // Both zero — the hidden-container case.
    Object.defineProperty(fader, 'clientHeight', { value: 0, configurable: true });
    Object.defineProperty(thumb, 'offsetHeight', { value: 0, configurable: true });
    return { fader, thumb };
  }

  it('leaves the thumb untouched rather than pinning it to the top', () => {
    const { fader, thumb } = unlaidPair();
    paintFader(fader, thumb, 0.25);
    // Writing `top: 0px` here is what read as "full scale"; not writing at all
    // leaves the thumb wherever it was until a real repaint arrives.
    expect(thumb.style.top).toBe('');
  });

  it('still records the norm, so the lit fill stays correct', () => {
    const { fader, thumb } = unlaidPair();
    paintFader(fader, thumb, 0.25);
    expect(fader.style.getPropertyValue('--fader-norm')).toBe('0.25');
  });

  it('does not clobber a previously-painted position', () => {
    const fader = document.createElement('div');
    const thumb = document.createElement('div');
    fader.appendChild(thumb);
    document.body.appendChild(fader);
    Object.defineProperty(fader, 'clientHeight', { value: 100, configurable: true });
    Object.defineProperty(thumb, 'offsetHeight', { value: 20, configurable: true });
    paintFader(fader, thumb, 0.5);
    const painted = thumb.style.top;
    expect(painted).not.toBe('');

    // The container is then hidden and repainted (e.g. a tab flip away).
    Object.defineProperty(fader, 'clientHeight', { value: 0, configurable: true });
    Object.defineProperty(thumb, 'offsetHeight', { value: 0, configurable: true });
    paintFader(fader, thumb, 0.9);
    expect(thumb.style.top).toBe(painted);
  });
});
