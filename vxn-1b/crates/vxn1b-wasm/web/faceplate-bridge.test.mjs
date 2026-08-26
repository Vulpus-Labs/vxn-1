// Headless test for the faceplate bridge (0291).
//
//   cargo build -p vxn1b-web-controller --target wasm32-unknown-unknown --release
//   node --test vxn-1b/crates/vxn1b-wasm/web/faceplate-bridge.test.mjs
//
// Drives the REAL controller wasm through the bridge, with a recording stand-in
// for the coordinator so every ring push is observable. FAILS (never skips) if
// the wasm is not built.

import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { FaceplateBridge, routeOpcode } from "./faceplate-bridge.mjs";
import { WebController } from "./controller.mjs";
import { ParamStore, createParamSAB, TOTAL_PARAMS, patchClapId } from "./param-store.mjs";
import {
  LAYER_L1,
  LAYER_L2,
  MATRIX_SLOTS,
  MATRIX_FIELD_DEST,
  MATRIX_FIELD_SOURCE,
} from "./event-codec.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const WASM = path.resolve(
  here,
  "../../../../target/wasm32-unknown-unknown/release/vxn1b_web_controller.wasm",
);
const wasmBytes = await readFile(WASM).catch(() => {
  throw new Error(
    `controller wasm not built at ${WASM}\n` +
      "run: cargo build -p vxn1b-web-controller --target wasm32-unknown-unknown --release",
  );
});

const CUTOFF = 19; // PATCH_PARAMS index; see controller.test.mjs

/// Records every ring push the bridge makes, in order.
class FakeCoordinator {
  constructor() {
    this.calls = [];
  }
  setKeyMode(mode) {
    this.calls.push(["keyMode", mode]);
  }
  setSplitPoint(note) {
    this.calls.push(["splitPoint", note]);
  }
  setLfo2Link(on) {
    this.calls.push(["lfo2Link", on]);
  }
  setMatrix(layer, slot, field, value) {
    this.calls.push(["matrix", layer, slot, field, value]);
  }
  setScopeTap(tap) {
    this.calls.push(["scopeTap", tap]);
  }
  setTempo(bpm) {
    this.calls.push(["setTempo", bpm]);
  }
  pollMeters() {
    return this.meters ?? null;
  }
  pollScope() {
    return this.scope ?? null;
  }
  of(kind) {
    return this.calls.filter((c) => c[0] === kind);
  }
  clear() {
    this.calls.length = 0;
  }
}

/// A `window` stand-in carrying the page's two entry points.
function fakeWin() {
  const batches = [];
  const corpora = [];
  return {
    __vxn: {
      applyViewEvents: (arr) => batches.push(arr),
      applyPresetCorpus: (snap) => corpora.push(snap),
    },
    batches,
    corpora,
    flat: () => batches.flat(),
  };
}

async function rig() {
  const store = new ParamStore(createParamSAB());
  const controller = await new WebController({ wasmBytes, store }).instantiate();
  const coordinator = new FakeCoordinator();
  const win = fakeWin();
  const bridge = new FaceplateBridge({ controller, coordinator, win });
  return { controller, coordinator, win, bridge, store };
}

// ---- routing table ---------------------------------------------------------

test("params and gestures go to the controller only, never the ring", async () => {
  const { bridge, coordinator, controller } = await rig();
  const id = patchClapId(LAYER_L1, CUTOFF);
  bridge.handle({ op: "begin_gesture", id });
  bridge.handle({ op: "set_param", id, plain: 900 });
  bridge.handle({ op: "end_gesture", id });
  bridge.handle({ op: "set_param_norm", id, norm: 0.25 });
  assert.deepEqual(coordinator.calls, [], "params/gestures must not touch the ring");
  // …and they did reach the model.
  const evs = controller.tick();
  assert.ok(evs.some((e) => e.kind === "param_changed" && e.id === id));
  controller.destroy();
});

test("non-param state reaches the model at route time and the ring on the pump", async () => {
  const { bridge, coordinator, controller } = await rig();
  bridge.pump(); // boot seed
  coordinator.clear();

  bridge.handle({ op: "set_key_mode", mode: 2 });
  bridge.handle({ op: "set_split_point", note: 48 });
  bridge.handle({ op: "set_lfo2_link", on: true });
  bridge.handle({ op: "set_matrix", layer: "lower", slot: 3, field: "dest", value: 5 });

  // Routing alone pushes NOTHING: these live in the model, so the ring is fed
  // by the resend, once, on the next pump. Double-pushing was the bug this
  // test's sibling ("only fields that actually moved") caught.
  assert.deepEqual(coordinator.calls, [], "route time must not push to the ring");

  bridge.pump();

  // Ring half — the assertion 0290 could not make, having no ring.
  assert.deepEqual(coordinator.of("keyMode"), [["keyMode", 2]]);
  assert.deepEqual(coordinator.of("splitPoint"), [["splitPoint", 48]]);
  assert.deepEqual(coordinator.of("lfo2Link"), [["lfo2Link", true]]);
  assert.deepEqual(coordinator.of("matrix"), [
    ["matrix", LAYER_L2, 3, MATRIX_FIELD_DEST, 5],
  ]);

  // Model half.
  const keys = controller.tick().find((e) => e.kind === "keys");
  assert.equal(keys, undefined, "the model already echoed on the previous pump");
  controller.destroy();
});

test("the scope tap is ring-only and never reaches the model", async () => {
  const { bridge, coordinator, controller } = await rig();
  controller.tick();
  const before = controller.snapshotState();
  bridge.handle({ op: "set_scope_source", source: "lower" });
  assert.deepEqual(coordinator.of("scopeTap"), [["scopeTap", 2]]);
  assert.deepEqual(controller.tick(), [], "a scope tap produced view events");
  assert.deepEqual(
    [...controller.snapshotState()],
    [...before],
    "a scope tap mutated the patch",
  );
  controller.destroy();
});

test("an unknown, non-string or malformed op is dropped, not mis-routed", async () => {
  const { bridge, coordinator, controller } = await rig();
  const before = controller.snapshotState();
  assert.equal(bridge.handle({ op: "no_such_opcode", id: 1 }), false);
  // vxn-2's page posts a numeric `op` for its operator tab; VXN1b's never does,
  // so a number here is a bug, not a case to handle.
  assert.equal(bridge.handle({ op: 3 }), false);
  assert.equal(bridge.handle(null), false);
  assert.equal(bridge.handle({}), false);
  // Unknown enum members inside a known op are refused rather than coerced.
  assert.equal(bridge.handle({ op: "set_matrix", layer: "sideways", slot: 0, field: "dest", value: 1 }), false);
  assert.equal(bridge.handle({ op: "set_matrix", layer: "upper", slot: 0, field: "wobble", value: 1 }), false);
  assert.equal(bridge.handle({ op: "set_scope_source", source: "elsewhere" }), false);
  assert.equal(bridge.handle({ op: "copy_layer", from: "upper", to: "sideways" }), false);

  assert.deepEqual(coordinator.calls, []);
  assert.deepEqual([...controller.snapshotState()], [...before]);
  controller.destroy();
});

test("the two known-dead fork opcodes are dropped without a warning", async () => {
  const { bridge, coordinator, controller } = await rig();
  // reset_layer / set_edit_layer: see ticket 0307. Dropped like any unhandled
  // op, but deliberately — the web build must not invent behaviour the plugin
  // does not have.
  assert.equal(bridge.handle({ op: "reset_layer", layer: "upper" }), false);
  assert.equal(bridge.handle({ op: "set_edit_layer", layer: "lower" }), false);
  assert.deepEqual(coordinator.calls, []);
  controller.destroy();
});

test("routeOpcode works without a coordinator (headless / audio not started)", async () => {
  const { controller } = await rig();
  assert.equal(routeOpcode(controller, null, { op: "set_key_mode", mode: 1 }), true);
  assert.equal(routeOpcode(controller, null, { op: "set_scope_source", source: "off" }), true);
  controller.destroy();
});

// ---- the echo-driven engine resync ----------------------------------------

test("the first pump seeds the ring with the whole topology and key state", async () => {
  const { bridge, coordinator, controller } = await rig();
  bridge.pump();
  // 2 layers x 16 slots x 4 fields, from a cold memo.
  assert.equal(coordinator.of("matrix").length, 2 * MATRIX_SLOTS * 4);
  assert.equal(coordinator.of("keyMode").length, 1);
  assert.equal(coordinator.of("splitPoint").length, 1);
  assert.equal(coordinator.of("lfo2Link").length, 1);
  // 131 events, against a 1024-slot ring — one block, no bulk tag needed.
  assert.equal(coordinator.calls.length, 131);

  // Nothing moved → the second pump pushes nothing.
  coordinator.clear();
  bridge.pump();
  assert.deepEqual(coordinator.calls, [], "a quiet pump must not re-push");
  controller.destroy();
});

test("a preset load resends topology to the ring, not just params", async () => {
  const { bridge, coordinator, controller, store } = await rig();
  bridge.pump(); // boot seed
  coordinator.clear();

  // Make the live topology differ from the preset's so the resend has work.
  bridge.handle({ op: "set_matrix", layer: "upper", slot: 0, field: "source", value: 4 });
  bridge.pump();
  coordinator.clear();

  bridge.handle({ op: "load_factory", index: 0 });
  bridge.pump();

  const pushes = coordinator.of("matrix");
  assert.ok(pushes.length > 0, "a preset load pushed no topology to the ring");
  // The engine also got the params, through the mirror.
  assert.ok(Number.isFinite(store.read(patchClapId(LAYER_L1, CUTOFF))));
  controller.destroy();
});

test("copy_layer reaches the engine as params + a topology resend", async () => {
  const { bridge, coordinator, controller, store } = await rig();
  bridge.pump();
  bridge.handle({ op: "set_param", id: patchClapId(LAYER_L1, CUTOFF), plain: 950 });
  bridge.handle({ op: "set_matrix", layer: "upper", slot: 5, field: "source", value: 3 });
  bridge.pump();
  coordinator.clear();

  bridge.handle({ op: "copy_layer", from: "upper", to: "lower" });
  bridge.pump();

  // Params: through the store mirror.
  const l2 = patchClapId(LAYER_L2, CUTOFF);
  assert.ok(Math.abs(store.read(l2) - 950) < 1, "copy_layer did not reach the SAB");
  // Topology: through the echo resend, on layer 2's slot 5.
  const pushed = coordinator
    .of("matrix")
    .filter((c) => c[1] === LAYER_L2 && c[2] === 5 && c[3] === MATRIX_FIELD_SOURCE);
  assert.deepEqual(pushed, [["matrix", LAYER_L2, 5, MATRIX_FIELD_SOURCE, 3]]);
  controller.destroy();
});

test("the resend pushes only fields that actually moved", async () => {
  const { bridge, coordinator, controller } = await rig();
  bridge.pump();
  coordinator.clear();
  bridge.handle({ op: "set_matrix", layer: "lower", slot: 7, field: "curve", value: 2 });
  bridge.pump();
  // One field changed → exactly one ring push: not a whole-table resend, and
  // not two (the route-time + resend double-push this caught).
  assert.equal(coordinator.of("matrix").length, 1);
  controller.destroy();
});

test("topology is pushed to the ring BEFORE depths are mirrored", async () => {
  // The ordering the harmful tear depends on: the worklet reads the store
  // first and the ring second, so the ring push must happen first. See the
  // note at the top of faceplate-bridge.mjs and audio-host.mjs process().
  const order = [];
  const store = new ParamStore(createParamSAB());
  const controller = await new WebController({ wasmBytes, store }).instantiate();
  const coordinator = new FakeCoordinator();
  const origSetMatrix = coordinator.setMatrix.bind(coordinator);
  coordinator.setMatrix = (...a) => {
    order.push("ring");
    origSetMatrix(...a);
  };
  const origMirror = controller.mirrorToStore.bind(controller);
  controller.mirrorToStore = () => {
    order.push("mirror");
    return origMirror();
  };
  const bridge = new FaceplateBridge({ controller, coordinator, win: fakeWin() });
  bridge.pump();
  assert.equal(order[0], "ring", `expected ring first, got ${order.slice(0, 3)}`);
  assert.equal(order[order.length - 1], "mirror", "the mirror must be last");
  controller.destroy();
});

// ---- the page-facing side --------------------------------------------------

test("the ipc shim parses and routes; junk JSON is survived", async () => {
  const { bridge, controller, coordinator, win } = await rig();
  bridge.install();
  assert.equal(typeof win.ipc.postMessage, "function");
  bridge.pump(); // boot seed, so the resend below is provably the new edit
  coordinator.clear();
  win.ipc.postMessage(JSON.stringify({ op: "set_key_mode", mode: 1 }));
  bridge.pump();
  assert.deepEqual(coordinator.of("keyMode"), [["keyMode", 1]]);
  // Must not throw the page's sender.
  win.ipc.postMessage("{not json");
  controller.destroy();
});

test("a pump delivers one batch to applyViewEvents", async () => {
  const { bridge, controller, win } = await rig();
  bridge.pump();
  assert.equal(win.batches.length, 1, "expected exactly one applyViewEvents call");
  const kinds = win.batches[0].map((e) => e.kind);
  assert.ok(kinds.includes("matrix"));
  assert.ok(kinds.includes("keys"));
  controller.destroy();
});

test("telemetry rides the same batch as the controller's events", async () => {
  const { bridge, coordinator, controller, win } = await rig();
  // MeterTap order: l1 L/R, l2 L/R, dynIn L/R, dynOut L/R, gr, master L/R.
  coordinator.meters = Float32Array.from([
    0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8, -3.5, 0.9, 1.0,
  ]);
  coordinator.scope = Float32Array.from([0, 0.5, -0.5, 9.0]);
  bridge.pump();

  const batch = win.batches[0];
  const m = batch.find((e) => e.kind === "meters");
  assert.ok(m, "no meter frame in the batch");
  assert.deepEqual(m.l1, [0.1, 0.2].map(Math.fround));
  assert.deepEqual(m.l2, [0.3, 0.4].map(Math.fround));
  assert.deepEqual(m.dynIn, [0.5, 0.6].map(Math.fround));
  assert.deepEqual(m.dynOut, [0.7, 0.8].map(Math.fround));
  assert.equal(m.dynGr, Math.fround(-3.5), "gain reduction is one value, not a pair");
  assert.deepEqual(m.master, [0.9, 1.0].map(Math.fround));

  const s = batch.find((e) => e.kind === "scope");
  assert.ok(s, "no scope frame in the batch");
  assert.equal(s.s.length, 4);
  assert.equal(s.s[2], -0.5);
  assert.equal(s.s[3], 2, "samples past the rails must clamp, not bloat the frame");
  controller.destroy();
});

test("the corpus is published on a corpus change, and readable at boot", async () => {
  const { bridge, controller, win } = await rig();
  bridge.publishCorpus();
  assert.equal(win.corpora.length, 1);
  assert.ok(win.corpora[0].factory.length > 0, "embedded bank missing at boot");

  bridge.handle({ op: "save_preset", name: "Mine", folder: null });
  bridge.pump();
  assert.equal(win.corpora.length, 2, "a save did not republish the corpus");
  assert.ok(JSON.stringify(win.corpora[1]).includes("Mine"));
  controller.destroy();
});

test("request_text_input is answered in-page and never reaches the controller", async () => {
  const { bridge, controller, win } = await rig();
  bridge.setPrompt(() => "typed name");
  bridge.handle({ op: "request_text_input", id: "ti1", title: "Name", initial: "" });
  const ev = win.flat().find((e) => e.kind === "text_input_result");
  assert.deepEqual(ev, { kind: "text_input_result", id: "ti1", value: "typed name" });
  // Nothing was posted to the model.
  assert.deepEqual(controller.tick().filter((e) => e.kind === "status"), []);
  controller.destroy();
});

test("a cancelled prompt delivers null, matching the native contract", async () => {
  const { bridge, controller, win } = await rig();
  bridge.setPrompt(() => null);
  bridge.handle({ op: "request_text_input", id: "ti2", title: "Name", initial: "x" });
  const ev = win.flat().find((e) => e.kind === "text_input_result");
  assert.equal(ev.value, null);
  controller.destroy();
});

test("the flush hook owns the drain; without one the pump drains anyway", async () => {
  const store = new ParamStore(createParamSAB());
  const controller = await new WebController({ wasmBytes, store }).instantiate();

  // With a hook, the pump must NOT drain — PresetPersistence.flush() does, and
  // a pump that drained first would leave it nothing to write.
  const seen = [];
  const bridge = new FaceplateBridge({
    controller,
    coordinator: new FakeCoordinator(),
    win: fakeWin(),
    onFlushJournal: () => seen.push(...controller.takeJournal()),
  });
  bridge.handle({ op: "save_preset", name: "Journalled", folder: null });
  bridge.pump();
  assert.ok(seen.some((o) => o.key.includes("Journalled")), "the save did not reach the hook");

  // With no hook it still drains, or the wasm journal grows without bound.
  const bare = new FaceplateBridge({ controller, win: fakeWin() });
  bare.handle({ op: "save_preset", name: "Dropped", folder: null });
  bare.pump();
  assert.deepEqual(controller.takeJournal(), [], "journal was not drained without a hook");
  controller.destroy();
});

test("a pump that throws does not kill the loop", async () => {
  const { bridge, controller } = await rig();
  let frames = 0;
  const raf = (fn) => {
    if (frames++ < 3) fn();
  };
  bridge._raf = raf;
  controller.tick = () => {
    throw new Error("boom");
  };
  const errs = [];
  const origErr = console.error;
  console.error = (...a) => errs.push(a);
  try {
    bridge.start();
  } finally {
    console.error = origErr;
    bridge.stop();
  }
  assert.ok(frames >= 3, "the rAF chain died on the first throw");
  assert.ok(errs.length >= 1, "the failure was swallowed silently");
});

// ---- boot behaviours -------------------------------------------------------

test("install drains the opcodes the page queued before the module loaded", async () => {
  const { bridge, controller, win, coordinator } = await rig();
  // What WEB_BOOT_HEAD's stub does during page parse: buffer raw JSON. `ready`
  // is the one that matters — it carries the full re-broadcast that paints
  // every control, so dropping the queue means a page that comes up blank.
  win.__VXN_UI_QUEUE__ = [
    JSON.stringify({ op: "set_key_mode", mode: 1 }),
    JSON.stringify({ op: "ready" }),
  ];
  bridge.install();
  assert.deepEqual(win.__VXN_UI_QUEUE__, [], "the queue must be emptied, not copied");

  const evs = bridge.pump();
  const ids = new Set(evs.filter((e) => e.kind === "param_changed").map((e) => e.id));
  assert.equal(ids.size, TOTAL_PARAMS, "the queued `ready` did not re-broadcast");
  assert.deepEqual(coordinator.of("keyMode"), [["keyMode", 1]]);
  controller.destroy();
});

test("install splices the SAME array the stub closed over", async () => {
  const { bridge, controller, win } = await rig();
  const q = [JSON.stringify({ op: "ready" })];
  win.__VXN_UI_QUEUE__ = q;
  bridge.install();
  // The stub holds `q` directly; reassigning a fresh [] would leave it pushing
  // into an array nobody drains.
  assert.equal(win.__VXN_UI_QUEUE__, q);
  assert.equal(q.length, 0);
  controller.destroy();
});

test("install survives a missing or malformed queue", async () => {
  const { bridge, controller, win } = await rig();
  delete win.__VXN_UI_QUEUE__;
  bridge.install();
  win.__VXN_UI_QUEUE__ = "not an array";
  bridge.install();
  controller.destroy();
});

test("resyncEngine re-pushes the whole topology and every param slot", async () => {
  const { bridge, coordinator, controller, store } = await rig();
  bridge.pump(); // boot seed: full topology + a full mirror
  coordinator.clear();
  bridge.pump();
  assert.deepEqual(coordinator.calls, [], "quiescent before the resync");

  // What WebHost.start() does to us: overwrite the store with engine defaults.
  const id = patchClapId(LAYER_L1, CUTOFF);
  bridge.handle({ op: "set_param", id, plain: 913 });
  bridge.pump();
  store.write(id, 1.0); // stand-in for _seedStoreFromDefaults clobbering it

  bridge.resyncEngine();
  bridge.pump();

  assert.ok(
    Math.abs(store.read(id) - 913) < 1,
    "the resync did not rewrite a slot the audio graph had clobbered",
  );
  assert.equal(
    coordinator.of("matrix").length,
    2 * MATRIX_SLOTS * 4,
    "the resync did not re-push the whole topology",
  );
  controller.destroy();
});

// ---- the DOM text-input modal ---------------------------------------------

/// The smallest DOM that exercises the modal path — enough for createElement,
/// append, focus, remove and keydown listeners.
function fakeDoc() {
  const mk = (tag) => {
    const el = {
      tagName: tag,
      className: "",
      textContent: "",
      type: "",
      value: "",
      children: [],
      _listeners: {},
      append(...kids) {
        this.children.push(...kids);
        for (const k of kids) k.parent = this;
      },
      remove() {
        if (this.parent) this.parent.children = this.parent.children.filter((c) => c !== this);
        this.removed = true;
      },
      addEventListener(type, fn) {
        (this._listeners[type] ||= []).push(fn);
      },
      removeEventListener() {},
      focus() {
        this.focused = true;
      },
      select() {},
      fire(type, ev) {
        for (const fn of this._listeners[type] || []) fn(ev);
      },
    };
    return el;
  };
  const body = mk("body");
  return { createElement: mk, body, _root: () => body.children[0] };
}

async function modalRig() {
  const store = new ParamStore(createParamSAB());
  const controller = await new WebController({ wasmBytes, store }).instantiate();
  const win = fakeWin();
  win.document = fakeDoc();
  const bridge = new FaceplateBridge({ controller, coordinator: new FakeCoordinator(), win });
  return { bridge, controller, win };
}

const keyEv = (key) => ({ key, preventDefault() {}, stopPropagation() {} });

test("the text-input modal commits on Enter and uses the shipped CSS classes", async () => {
  const { bridge, controller, win } = await modalRig();
  bridge.handle({ op: "request_text_input", id: "ti1", title: "Preset name", initial: "Init" });

  const backdrop = win.document._root();
  assert.equal(backdrop.className, "vxn-ti-backdrop", "must use WEB_BOOT_HEAD's classes");
  const box = backdrop.children[0];
  assert.equal(box.className, "vxn-ti-box");
  const [label, input] = box.children;
  assert.equal(label.className, "vxn-ti-title");
  assert.equal(label.textContent, "Preset name");
  assert.equal(input.className, "vxn-ti-input");
  assert.equal(input.value, "Init", "the initial value must be seeded");
  assert.ok(input.focused, "the field must be focused or the user types into the faceplate");

  input.value = "Renamed";
  input.fire("keydown", keyEv("Enter"));
  const ev = win.flat().find((e) => e.kind === "text_input_result");
  assert.deepEqual(ev, { kind: "text_input_result", id: "ti1", value: "Renamed" });
  assert.ok(backdrop.removed, "the modal must be torn down");
  controller.destroy();
});

test("Escape and click-outside cancel with null, and answer exactly once", async () => {
  const { bridge, controller, win } = await modalRig();
  bridge.handle({ op: "request_text_input", id: "ti2", title: "x", initial: "" });
  const backdrop = win.document._root();
  backdrop.children[0].children[1].fire("keydown", keyEv("Escape"));
  assert.equal(win.flat().filter((e) => e.kind === "text_input_result")[0].value, null);

  const { bridge: b3, controller: c3, win: w3 } = await modalRig();
  b3.handle({ op: "request_text_input", id: "ti3", title: "x", initial: "" });
  const bd = w3.document._root();
  bd.fire("pointerdown", { target: bd });
  // A second cancel (Enter after the box is gone) must NOT deliver twice: the
  // page's promptText callback is fire-once and a second answer would be lost
  // or, worse, applied to a later prompt.
  bd.children[0].children[1].fire("keydown", keyEv("Enter"));
  assert.equal(w3.flat().filter((e) => e.kind === "text_input_result").length, 1);
  controller.destroy();
  c3.destroy();
});

test("keystrokes in the modal do not leak to the faceplate's shortcuts", async () => {
  const { bridge, controller, win } = await modalRig();
  bridge.handle({ op: "request_text_input", id: "ti4", title: "x", initial: "" });
  const input = win.document._root().children[0].children[1];
  let stopped = false;
  input.fire("keydown", { key: "a", preventDefault() {}, stopPropagation() { stopped = true; } });
  assert.ok(stopped, "typing a name must not trigger single-key shortcuts");
  controller.destroy();
});

test("with no document the prompt answers rather than hanging", async () => {
  const { bridge, controller, win } = await rig(); // no win.document
  bridge.handle({ op: "request_text_input", id: "ti5", title: "x", initial: "" });
  const ev = win.flat().find((e) => e.kind === "text_input_result");
  assert.deepEqual(ev, { kind: "text_input_result", id: "ti5", value: null });
  controller.destroy();
});

test("the gesture gate starts audio once and resyncs, then detaches", async () => {
  const { bridge, controller, win } = await modalRig();
  const { attachGestureGate } = await import("./faceplate-bridge.mjs");
  let starts = 0;
  const host = {
    start: async () => {
      starts++;
    },
  };
  let resynced = 0;
  bridge.resyncEngine = () => {
    resynced++;
    return bridge;
  };
  const listeners = {};
  win.document.addEventListener = (t, fn) => {
    (listeners[t] ||= []).push(fn);
  };
  win.document.removeEventListener = (t, fn) => {
    listeners[t] = (listeners[t] || []).filter((f) => f !== fn);
  };
  attachGestureGate(win, host, bridge);
  assert.ok(listeners.pointerdown?.length, "no pointer listener");
  assert.ok(listeners.keydown?.length, "no key listener — keyboard players get silence");

  // Hold the handler: firing it detaches, which empties the array.
  const onGesture = listeners.pointerdown[0];
  await onGesture();
  assert.equal(listeners.pointerdown.length, 0, "pointer listener must detach after firing");
  assert.equal(listeners.keydown.length, 0, "key listener must detach too");

  // A second gesture (a queued event, or the keydown that arrives with the
  // click) must not start a second context or resync again.
  await onGesture();
  assert.equal(starts, 1, "audio must start exactly once");
  assert.equal(resynced, 1, "the engine must be resynced exactly once");
  controller.destroy();
});

test("boot() stands the whole thing up", async () => {
  // The only test that RUNS boot(). Everything else exercises the pieces, which
  // is exactly how `boot()` shipped referencing WebController without importing
  // it — green suite, ReferenceError in the browser on the first page load.
  const { boot } = await import("./faceplate-bridge.mjs");
  const engineWasm = await readFile(
    path.resolve(here, "../../../../target/wasm32-unknown-unknown/release/vxn1b_wasm.wasm"),
  );
  const fetchImpl = async (url) => ({
    ok: true,
    arrayBuffer: async () => (String(url).includes("controller") ? wasmBytes : engineWasm),
  });

  const win = fakeWin();
  win.__VXN_UI_QUEUE__ = [JSON.stringify({ op: "ready" })];

  const { host, controller, bridge } = await boot({
    win,
    fetchImpl,
    autoGesture: false, // no document here, and no AudioContext to resume
  });

  // The ipc shim replaced the queuing stub, and the queue was drained.
  assert.equal(typeof win.ipc.postMessage, "function");
  assert.deepEqual(win.__VXN_UI_QUEUE__, []);

  // ONE store: the host allocated it, the controller mirrors into it.
  assert.equal(controller.store, host.store);

  // The corpus reached the page without any fetch of a factory asset.
  assert.ok(win.corpora.length >= 1);
  assert.ok(win.corpora[0].factory.length > 0);

  // The queued `ready` re-broadcast every param on the first pump. Driven by
  // hand: this fake window has no requestAnimationFrame, so `start()` warned
  // and did not arm the loop.
  bridge.pump();
  bridge.stop();
  const ids = new Set(win.flat().filter((e) => e.kind === "param_changed").map((e) => e.id));
  assert.equal(ids.size, TOTAL_PARAMS, "boot did not paint the faceplate");

  // And the model's values reached the store the worklet will read.
  assert.ok(Number.isFinite(host.store.read(patchClapId(LAYER_L1, CUTOFF))));
  controller.destroy();
});

test("boot attaches the computer keyboard before audio exists", async () => {
  // The keyboard must be live BEFORE the gesture: the keypress that wakes the
  // context should also sound a note, which it can, because notes pushed before
  // `ready` wait in the ring and apply on the first live quantum. Attaching
  // after start() would swallow exactly that keystroke.
  const { boot } = await import("./faceplate-bridge.mjs");
  const engineWasm = await readFile(
    path.resolve(here, "../../../../target/wasm32-unknown-unknown/release/vxn1b_wasm.wasm"),
  );
  const fetchImpl = async (url) => ({
    ok: true,
    arrayBuffer: async () => (String(url).includes("controller") ? wasmBytes : engineWasm),
  });

  const listeners = {};
  const win = fakeWin();
  win.document = {
    addEventListener: (t, fn) => ((listeners[t] ||= []).push(fn)),
    removeEventListener: () => {},
    createElement: () => ({ append() {}, addEventListener() {}, focus() {}, select() {}, remove() {} }),
    body: { append() {} },
  };

  // Inject the REAL shared adapter: dist/ is flat so the production dynamic
  // import resolves there, but from the source tree it is two roots away.
  const { attachKeyboard } = await import(
    "../../../../crates/vxn-core-web/assets/keyboard-input.mjs"
  );
  const { bridge, controller, inputs } = await boot({
    win,
    fetchImpl,
    adapters: { attachKeyboard },
    autoGesture: false, // the MIDI half waits for a gesture; not exercised here
  });
  bridge.stop();

  assert.ok(inputs, "boot did not attach the keyboard adapter");
  assert.ok(listeners.keydown?.length, "no keydown listener — nothing can play the synth");
  controller.destroy();
});

test("set_tempo is ring-only and refuses a nonsense BPM", async () => {
  const { bridge, coordinator, controller } = await rig();
  controller.tick();
  const before = controller.snapshotState();

  bridge.handle({ op: "set_tempo", bpm: 96 });
  assert.deepEqual(coordinator.calls, [["setTempo", 96]]);
  // Tempo is not part of the patch — a preset must not carry the BPM you
  // happened to be at when you saved it.
  assert.deepEqual([...controller.snapshotState()], [...before]);
  assert.deepEqual(controller.tick(), [], "set_tempo produced view events");

  coordinator.clear();
  for (const bad of [0, -1, NaN, Infinity, "fast", undefined]) {
    assert.equal(bridge.handle({ op: "set_tempo", bpm: bad }), false, `accepted ${bad}`);
  }
  assert.deepEqual(coordinator.calls, [], "a nonsense tempo reached the engine");
  controller.destroy();
});

test("boot mounts the on-screen piano, and it plays into the ring", async () => {
  // VXN1b's faceplate has no playable keys of its own, so in a browser this is
  // the only way to sound a note without MIDI hardware or the QWERTY mapping.
  const { boot } = await import("./faceplate-bridge.mjs");
  const { createPianoKeyboard } = await import(
    "../../../../crates/vxn-core-web/assets/piano-keyboard.mjs"
  );
  const engineWasm = await readFile(
    path.resolve(here, "../../../../target/wasm32-unknown-unknown/release/vxn1b_wasm.wasm"),
  );
  const fetchImpl = async (url) => ({
    ok: true,
    arrayBuffer: async () => (String(url).includes("controller") ? wasmBytes : engineWasm),
  });

  // Enough DOM for the widget to build itself and be appended.
  const made = [];
  const mkEl = () => {
    const el = {
      style: {}, dataset: {}, children: [],
      set className(v) { this._c = v; }, get className() { return this._c; },
      appendChild(k) { this.children.push(k); return k; },
      addEventListener() {}, removeEventListener() {}, remove() {},
    };
    made.push(el);
    return el;
  };
  const doc = {
    body: mkEl(),
    getElementById: () => null,
    createElement: mkEl,
    addEventListener() {},
    removeEventListener() {},
  };
  const win = { document: doc, __vxn: { applyViewEvents() {}, applyPresetCorpus() {} },
                location: { hash: "" }, history: { replaceState() {} } };

  const { piano, host, controller, bridge } = await boot({
    win,
    fetchImpl,
    autoGesture: false,
    autoInputs: false,
    autoPersist: false,
    adapters: { createPianoKeyboard },
  });
  bridge.stop();

  assert.ok(piano, "boot did not mount the piano");
  // Three octaves C3..C6 = 37 keys, 22 white + 15 black.
  const keys = made.filter((e) => e.dataset && e.dataset.note != null);
  assert.equal(keys.length, 37, `expected 37 keys, got ${keys.length}`);

  // Pressing a key must reach the ring like any other producer. The ring is the
  // host's, so read the write index before and after.
  const before = Atomics.load(host.ring.ctrl, 0);
  piano._press(60);
  piano._release();
  const after = Atomics.load(host.ring.ctrl, 0);
  assert.ok(after > before, "a piano press pushed nothing onto the ring");
  controller.destroy();
});
