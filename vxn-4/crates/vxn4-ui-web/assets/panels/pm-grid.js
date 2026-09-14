// The 8×8 PM grid — vxn-4's own primitive, with no precedent to port.
//
// Rows are destinations, columns sources; the diagonal is an operator's own
// feedback and the last column is its send into the stereo sum bus. It is the
// one view where the whole topology is visible at once.
//
// The grid is editable, not just a map:
//
//   click        select the cell's SOURCE operator — whose route table the
//                panel below edits — and name the row it points at
//   drag ↕       set that route's constant, shift for fine
//   double-click toggle the route on or off
//
// which makes it the fast way to rough out a topology — wire a pair, pull its
// depth, move on — and leaves the table below for the parts a cell cannot
// hold: the offset and scaling sources and their curves. Both are views of the
// same route, so an edit here writes through to the table's controls rather
// than rebuilding it; a rebuild mid-drag would drop the pointer capture and
// lose the gesture.

import { formatValue } from './value.js';
import { attachPop, track, refreshPop, hidePop } from './pop.js';
import { cellDiv } from './dom.js';
import { wireDrag } from '../../../../../crates/vxn-core-ui-web/assets/wire-drag.js';

/* PM depths are bipolar, and the sign is not cosmetic — a modulator inverted
   against its neighbour cancels where it would otherwise reinforce, which is
   the difference between a patch and the same patch gone thin. So the grid
   encodes it twice:

     hue        blue for a positive depth, violet for a negative one
     accent bar along the cell's bottom edge for positive, top edge for
                negative — the same up/down sense as the bipolar fader the
                depth is dragged on

   Two channels rather than one because hue alone fails for a red-green or
   blue-violet colour-blind player, and the sign is exactly the thing that is
   invisible in the sound until you go looking for it. */
export const PM_TINT_POS = "108, 177, 255";   // operator → operator, positive depth
export const PM_TINT_NEG = "198, 122, 232";   // operator → operator, negative depth
export const BUS_TINT = "217, 112, 27";       // operator → sum bus (unipolar)

/**
 * How one cell paints, as a pure function of its route — so the encoding can
 * be asserted without a DOM.
 *
 * Two things are drawn, and they answer different questions:
 *
 *   the OUTLINE says a route EXISTS — enabled, whatever its depth. A route
 *   authored at 2% is still part of the patch's shape and has to be findable.
 *   the FILL is the authored depth, so the topology reads at a glance.
 *
 * Both are static patch data. The grid deliberately does NOT try to show live
 * signal: vxn-4 is polyphonic, so there is no single level per route — eight
 * voices each have their own, at different points in their own envelopes, and
 * any summary of them (max? mean? last voice?) would be a number the player
 * cannot act on.
 */
export function pmCellStyle(k, on, bus) {
  if (!on) return { boxShadow: "", background: "" };
  const signed = bus ? 0 : k - 0.5;            // bus sends are unipolar
  const depth = bus ? k : Math.abs(signed) * 2;
  const tint = bus ? BUS_TINT : (signed < 0 ? PM_TINT_NEG : PM_TINT_POS);
  const outline = `inset 0 0 0 1px rgba(${tint}, 0.62)`;
  // A depth sitting on zero gets no accent bar — it has no sign to report.
  const bar = bus || depth < 0.02
    ? ""
    : `, inset 0 ${signed < 0 ? "" : "-"}3px 0 -1px rgba(${tint}, 0.95)`;
  return {
    boxShadow: outline + bar,
    background: depth > 0.01 ? `rgba(${tint}, ${(0.14 + 0.82 * depth).toFixed(3)})` : "",
  };
}

/**
 * Build the grid into `el`.
 *
 * `model` supplies the routes: `route(src, dst)` for the bank, `bus(op)` for
 * the send column; each is an object with `.k` (normalised constant) and `.on`.
 * `pmSpec` / `busSpec` are the same specs the route table uses, so a depth
 * dragged here and the same depth read there are one number in one unit.
 *
 * `onSelect(srcOp, rowKey)` fires on a click that never became a drag;
 * `onEdit(route)` fires on every depth change and every toggle, and is how the
 * route table's controls are kept in step.
 *
 * Returns `{ paint, refreshSelection }`. Neither rebuilds: the cells have to
 * outlive a selection change, because rebuilding between the two clicks of a
 * double-click would replace the element under the pointer and the `dblclick`
 * would never land.
 */
export function createPmGrid(el, { nOps, model, pmSpec, busSpec, onSelect, onEdit }) {
  const cells = [];   // { el, route, bus, srcOp }

  el.innerHTML = "";
  el.append(cellDiv("pm-hd", ""));
  for (let s = 0; s < nOps; s++) el.append(cellDiv("pm-hd", String(s + 1)));
  el.append(cellDiv("pm-hd", ""));
  el.append(cellDiv("pm-hd", "BUS"));

  for (let d = 0; d < nOps; d++) {
    el.append(cellDiv("pm-rowhd", String(d + 1)));
    for (let s = 0; s < nOps; s++) {
      const route = model.route(s, d);
      const c = document.createElement("div");
      c.className = "pm-cell" + (s === d ? " diag" : "");
      wire(c, route, s === d ? `OP${s + 1} self` : `OP${s + 1} → OP${d + 1}`, pmSpec, s, d);
      el.append(c);
      cells.push({ el: c, route, bus: false, srcOp: s });
    }
    el.append(cellDiv("pm-hd", ""));
    const route = model.bus(d);
    const b = document.createElement("div");
    b.className = "pm-cell bus";
    wire(b, route, `OP${d + 1} → bus`, busSpec, d, "bus");
    el.append(b);
    cells.push({ el: b, route, bus: true, srcOp: d });
  }

  function wire(cell, route, label, spec, srcOp, rowKey) {
    attachPop(cell, () =>
      label + "  " + formatValue(spec, route.k) + (route.on ? "" : "   (off)"));

    let moved = false;
    wireDrag(
      cell,
      { downContext: () => route.k, axis: "y", shift: 0.25 },
      {
        onDown: (ev) => { moved = false; track(cell, ev); },
        onMove: (ev, { dy, ctx }) => {
          // A few pixels of slop before a click becomes a drag, so selecting a
          // cell does not nudge its depth on the way past.
          if (!moved && Math.abs(dy) < 3) return;
          moved = true;
          route.k = Math.min(1, Math.max(0, ctx - dy / 140));
          if (onEdit) onEdit(route);
          paint();
          track(cell, ev);
          refreshPop();
        },
        onUp: () => {
          hidePop(cell);
          // A click that never became a drag selects, which is what it did
          // before the cell became editable.
          if (!moved && onSelect) onSelect(srcOp, rowKey);
        },
        onDoubleClick: () => {
          if (moved) return;
          route.on = !route.on;
          if (onEdit) onEdit(route);
          paint();
          refreshPop();
        },
      },
    );
  }

  function paint() {
    cells.forEach(({ el: cell, route, bus }) => {
      const style = pmCellStyle(route.k, route.on, bus);
      cell.style.boxShadow = style.boxShadow;
      cell.style.background = style.background;
    });
  }

  function refreshSelection(selOp) {
    cells.forEach(({ el: cell, srcOp }) => cell.classList.toggle("sel-src", srcOp === selOp));
  }

  paint();
  return { paint, refreshSelection };
}
