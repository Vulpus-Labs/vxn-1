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

test("journal ops are drained every pump, with or without a sink", async () => {
  const store = new ParamStore(createParamSAB());
  const controller = await new WebController({ wasmBytes, store }).instantiate();
  const seen = [];
  const bridge = new FaceplateBridge({
    controller,
    coordinator: new FakeCoordinator(),
    win: fakeWin(),
    onJournal: (ops) => seen.push(...ops),
  });
  bridge.handle({ op: "save_preset", name: "Journalled", folder: null });
  bridge.pump();
  assert.ok(seen.some((o) => o.key.includes("Journalled")), "the save did not reach the sink");

  // With no sink the journal must still drain, or the wasm buffer grows forever.
  const bare = new FaceplateBridge({ controller, win: fakeWin() });
  bridge.handle({ op: "save_preset", name: "Dropped", folder: null });
  bare.pump();
  assert.deepEqual(controller.takeJournal(), [], "journal was not drained without a sink");
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
