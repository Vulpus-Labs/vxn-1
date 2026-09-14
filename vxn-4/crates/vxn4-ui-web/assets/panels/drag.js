// Continuous-control drag, over the shared `wireDrag` primitive.
//
// `wireDrag` (vxn-core-ui-web/assets/wire-drag.js) owns the mechanics every
// synth kept re-implementing: pointer capture, the `dragging` class, the
// shift-scaled delta-since-grab, the cancel path. What is left here is vxn-4's
// policy — travel in pixels per full range, and keeping the readout alive.
//
// The RELATIVE model, not the absolute one: a fader is grabbed wherever the
// pointer lands and moves from there, so the thumb never jumps to the pointer
// on pointerdown. That matters most on the PM grid, where a cell is 17px tall
// and an absolute mapping would make every click a full-range edit.

import { wireDrag } from '../../../../../crates/vxn-core-ui-web/assets/wire-drag.js';
import { track, refreshPop, hidePop } from './pop.js';

const clamp01 = (v) => (v < 0 ? 0 : v > 1 ? 1 : v);

/**
 * Drag `el` to move a normalised value.
 *
 * `axis` is "y" (faders, dials, grid cells) or "x" (route constants); `span`
 * is the pointer travel in pixels that covers the whole 0..1 range. Shift
 * gives quarter sensitivity — not `wireDrag`'s tenth, which on a 120px fader
 * makes the fine pass longer than the coarse one.
 */
export function dragNorm(el, { get, set, axis = "y", span = 140, onDoubleClick } = {}) {
  return wireDrag(
    el,
    { downContext: () => get(), axis, shift: 0.25 },
    {
      onDown: (ev) => track(el, ev),
      onMove: (ev, { dx, dy, ctx }) => {
        // Up is more, on every vertical control here.
        const delta = axis === "y" ? -dy : dx;
        set(clamp01(ctx + delta / span));
        track(el, ev);
        refreshPop();
      },
      onUp: () => hidePop(el),
      onDoubleClick,
    },
  );
}
