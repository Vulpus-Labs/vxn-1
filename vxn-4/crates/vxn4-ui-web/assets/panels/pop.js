// Popup readout, over the shared `valuePop` singleton.
//
// The house convention: nothing is printed on the faceplate, every readout
// follows the hand. `valuePop` (vxn-core-ui-web/assets/value-pop.js) owns the
// one `<div>`, its CSS ruleset and its fixed positioning — this module is only
// the per-control policy on top of it: which text, and when.
//
// It differs from vxn-1b's and vxn-2's use of the same singleton in one way,
// and it is the mockup's rule rather than an accident: the popup TRACKS the
// pointer instead of anchoring where the pointer entered. vxn-4's densest
// surface is the PM grid, where 17px cells sit 2px apart, and an anchored
// popup sits over the neighbouring cells you are about to read.
//
// One owner per element: `attach` stamps the text function on the element, and
// hover paints it. A drag keeps it up past `pointerleave` — the caller's
// `wireDrag` `onMove` calls `track`, `onUp` calls `hide`.

import { valuePop } from '../../../../../crates/vxn-core-ui-web/assets/value-pop.js';

// The element whose text the popup is currently showing. A control that is
// being dragged keeps ownership even while the pointer is over something else,
// which is what pointer capture means for the readout too.
let popOwner = null;

/** Register `textFn` as `el`'s readout and wire hover. */
export function attachPop(el, textFn) {
  el.popText = textFn;
  el.addEventListener("pointerenter", (ev) => track(el, ev));
  el.addEventListener("pointermove", (ev) => {
    // A drag on another control owns the popup; do not steal it just because
    // the pointer passed over this one.
    if (popOwner === null || popOwner === el) track(el, ev);
  });
  el.addEventListener("pointerleave", () => hidePop(el));
}

/** Show (or move) `el`'s readout at the pointer. */
export function track(el, ev) {
  if (!el.popText) return;
  popOwner = el;
  valuePop.show(el.popText(), ev.clientX, ev.clientY);
}

/** Repaint the current owner's text in place — a drag changing the value. */
export function refreshPop() {
  if (popOwner && popOwner.popText) valuePop.update(popOwner.popText());
}

/** Release the popup if `el` holds it. A control mid-drag keeps it. */
export function hidePop(el) {
  if (el && popOwner !== el) return;
  if (popOwner && popOwner.classList.contains("dragging")) return;
  valuePop.hide();
  popOwner = null;
}

/**
 * Drop the popup unconditionally. Called when a surface goes away under the
 * pointer — a tab switch, the matrix overlay closing — because the element
 * that owned the readout may never see its own `pointerleave`.
 */
export function clearPop() {
  valuePop.hide();
  popOwner = null;
}
