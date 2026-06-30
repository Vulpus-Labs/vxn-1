import { describe, it, expect, beforeEach } from "vitest";
// The KS graph rides the shared `wireDrag` primitive, which the production
// bundle exposes as a script-scope global (ESM marker stripped at splice). The
// suite imports it and pins it on globalThis so the IIFE-loaded ks-graph module
// resolves the same free `wireDrag` reference at drag time.
import { wireDrag } from "../../../../../crates/vxn-core-ui-web/assets/wire-drag.js";
globalThis.wireDrag = wireDrag;
import "../panels/ks-graph.js";

const { ksGraph } = window.__vxn.panels;

// IDs for the four KS params of op1.
const ID = { bp: 10, l: 11, r: 12, rate: 13 };

function makeVxn() {
  return {
    paramsByName: {
      "op1-ks-break-pt": { id: ID.bp, default: 60 },
      "op1-ks-l-depth": { id: ID.l, default: 0 },
      "op1-ks-r-depth": { id: ID.r, default: 0 },
      "op1-ks-rate": { id: ID.rate, default: 0 },
    },
    // identity-ish note name — the graph only needs *a* string per midi key.
    noteName: (m) => "N" + m,
    // op0 left = NegLin (0), right = NegExp (2) — the production defaults.
    ksCurves: [[0, 2]],
  };
}

function makeCtx(vxn) {
  const dispatched = [];
  const registered = [];
  const b = {
    op: 1,
    vxn,
    dispatch: (opcode, payload) => dispatched.push({ opcode, payload }),
    register: (id, prim, wrap) => registered.push({ id, prim, wrap }),
  };
  return { b, dispatched, registered };
}

function pointerEvt(type, { clientX = 0, clientY = 0, pointerId = 7 } = {}) {
  const ev = new MouseEvent(type, { bubbles: true, cancelable: true });
  Object.defineProperty(ev, "pointerId", { value: pointerId });
  Object.defineProperty(ev, "clientX", { value: clientX });
  Object.defineProperty(ev, "clientY", { value: clientY });
  return ev;
}

function lastOf(dispatched, opcode) {
  for (let i = dispatched.length - 1; i >= 0; i--) {
    if (dispatched[i].opcode === opcode) return dispatched[i];
  }
  return null;
}

describe("ks-graph — create", () => {
  let parent;
  beforeEach(() => {
    document.body.innerHTML = "";
    parent = document.createElement("div");
    document.body.appendChild(parent);
  });

  it("returns null when the op's KS params are absent", () => {
    const { b } = makeCtx({ paramsByName: {}, noteName: String, ksCurves: [[0, 2]] });
    expect(ksGraph.create(parent, b)).toBeNull();
  });

  it("builds the graph, registers all four KS params, exposes applyCurves", () => {
    const { b, registered } = makeCtx(makeVxn());
    const api = ksGraph.create(parent, b);
    expect(api).toBeTruthy();
    expect(typeof api.applyCurves).toBe("function");
    expect(registered.map((r) => r.id).sort()).toEqual([ID.bp, ID.l, ID.r, ID.rate]);
    expect(parent.querySelector(".op-ks-graph")).toBeTruthy();
    expect(parent.querySelectorAll("[data-ks-pt]")).toHaveLength(3);
  });
});

describe("ks-graph — break-point handle drag (horizontal)", () => {
  let parent;
  beforeEach(() => {
    document.body.innerHTML = "";
    parent = document.createElement("div");
    document.body.appendChild(parent);
  });

  it("maps dx·0.5 onto the break-point note, gesture-bracketed", () => {
    const { b, dispatched } = makeCtx(makeVxn());
    ksGraph.create(parent, b);
    const bp = parent.querySelector('[data-ks-pt="bp"]');
    bp.dispatchEvent(pointerEvt("pointerdown", { clientX: 100 }));
    // +40 px × 0.5 gain = +20 semis from the default break point (60) → 80.
    bp.dispatchEvent(pointerEvt("pointermove", { clientX: 140 }));
    bp.dispatchEvent(pointerEvt("pointerup", { clientX: 140 }));

    expect(lastOf(dispatched, "begin_gesture").payload).toEqual({ id: ID.bp });
    expect(lastOf(dispatched, "set_param").payload).toEqual({ id: ID.bp, plain: 80 });
    expect(lastOf(dispatched, "end_gesture").payload).toEqual({ id: ID.bp });
  });

  it("clamps the break point to [0,127]", () => {
    const { b, dispatched } = makeCtx(makeVxn());
    ksGraph.create(parent, b);
    const bp = parent.querySelector('[data-ks-pt="bp"]');
    bp.dispatchEvent(pointerEvt("pointerdown", { clientX: 0 }));
    bp.dispatchEvent(pointerEvt("pointermove", { clientX: 1000 })); // way past 127
    expect(lastOf(dispatched, "set_param").payload).toEqual({ id: ID.bp, plain: 127 });
  });
});

describe("ks-graph — depth handle drag (vertical, signed across the midline)", () => {
  let parent;
  beforeEach(() => {
    document.body.innerHTML = "";
    parent = document.createElement("div");
    document.body.appendChild(parent);
  });

  it("drag up boosts: flips the left curve's sign bit and sets the depth", () => {
    const { b, dispatched } = makeCtx(makeVxn());
    ksGraph.create(parent, b);
    const l = parent.querySelector('[data-ks-pt="l"]');
    // left default = NegLin (curve 0): sign bit clear → start signed depth 0.
    l.dispatchEvent(pointerEvt("pointerdown", { clientY: 100 }));
    // drag UP 40 px → up = +20 → signed +20 → depth 20, sign bit set (boost).
    l.dispatchEvent(pointerEvt("pointermove", { clientY: 60 }));
    l.dispatchEvent(pointerEvt("pointerup", { clientY: 60 }));

    // sign flipped 0 → 1 (boost), shape bit (0, lin) preserved.
    expect(lastOf(dispatched, "set_ks_curve").payload).toEqual({ op: 0, side: 0, curve: 1 });
    expect(lastOf(dispatched, "set_param").payload).toEqual({ id: ID.l, plain: 20 });
  });

  it("drag down cuts on the right side: keeps the cut sign, sets depth", () => {
    const { b, dispatched } = makeCtx(makeVxn());
    ksGraph.create(parent, b);
    const r = parent.querySelector('[data-ks-pt="r"]');
    // right default = NegExp (curve 2): sign bit clear (cut), shape bit set.
    r.dispatchEvent(pointerEvt("pointerdown", { clientY: 100 }));
    // drag DOWN 40 px → up = -20 → signed -20 → depth 20, sign bit clear (cut).
    r.dispatchEvent(pointerEvt("pointermove", { clientY: 140 }));
    r.dispatchEvent(pointerEvt("pointerup", { clientY: 140 }));

    // sign already clear, shape exp preserved → curve stays 2, no flip dispatch.
    expect(lastOf(dispatched, "set_param").payload).toEqual({ id: ID.r, plain: 20 });
  });
});

describe("ks-graph — shape toggles", () => {
  let parent;
  beforeEach(() => {
    document.body.innerHTML = "";
    parent = document.createElement("div");
    document.body.appendChild(parent);
  });

  it("the Lin/Exp button flips the shape bit and dispatches set_ks_curve", () => {
    const { b, dispatched } = makeCtx(makeVxn());
    ksGraph.create(parent, b);
    const lShape = parent.querySelector('[data-ks-shape="l"]');
    // left starts curve 0 (lin); click → exp = bit1 set = 2.
    lShape.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    expect(lastOf(dispatched, "set_ks_curve").payload).toEqual({ op: 0, side: 0, curve: 2 });
    expect(lShape.textContent).toBe("L exp");
  });
});
