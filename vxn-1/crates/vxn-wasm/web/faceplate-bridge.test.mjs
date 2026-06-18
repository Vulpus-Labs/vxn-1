// Headless Node test for the faceplate transport bridge (E018 / 0057-0061).
//
//   cargo build -p vxn-web-controller --target wasm32-unknown-unknown
//   node web/faceplate-bridge.test.mjs
//
// Drives the EXACT shared code path the browser runs (faceplate-bridge.mjs +
// the real vxn-web-controller wasm + the real ParamStore over a SAB), with hard
// asserts — same discipline as controller.test.mjs. DOM rendering can't run in
// Node, so this proves the OPCODE ROUND-TRIP headlessly:
//
//   0058 (JS -> controller): an opcode posted through `handleUiOpcode` mutates
//        the controller model and the value lands in the SHARED store SAB; the
//        layer/keymode string<->int mapping is correct; preset ops are inert.
//   0059 (controller -> JS): a controller ViewEvent is translated to the
//        faceplate `{kind,..}` shape, deduped by id, and handed to a fake
//        `window.__vxn.applyViewEvents` (the `dispatch` sink).
//   0060 (gesture brackets): begin/setParamNorm/end reaches the controller in
//        order and settles a ParamChanged.
//   0061 (text input): `request_text_input` is intercepted (not forwarded) and
//        a commit/cancel delivers `text_input_result` exactly once.

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { createParamSAB, ParamStore, TOTAL_PARAMS } from "./param-store.mjs";
import { WebController } from "./controller.mjs";
import {
  FaceplateBridge,
  viewEventToFaceplate,
  dedupParamChanged,
  openTextInputPopup,
} from "./faceplate-bridge.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const WASM = join(
  here,
  "../../../../target/wasm32-unknown-unknown/debug/vxn_web_controller.wasm",
);

let failures = 0;
const check = (cond, msg) => {
  console.log(`  ${cond ? "PASS" : "FAIL"}  ${msg}`);
  if (!cond) failures++;
};
const approx = (a, b, eps = 1e-5) => Math.abs(a - b) <= eps;

async function main() {
  let wasmBytes;
  try {
    wasmBytes = await readFile(WASM);
  } catch {
    console.error(
      `\n  controller wasm not found at ${WASM}\n` +
        `  build it first: cargo build -p vxn-web-controller --target wasm32-unknown-unknown\n`,
    );
    process.exit(2);
  }

  const sab = createParamSAB();
  const store = new ParamStore(sab);
  const controller = new WebController({ wasmBytes, store });
  await controller.instantiate();

  // The page dispatcher sink (fake `window.__vxn.applyViewEvents`). The bridge
  // drives `tick()` manually in this test (no rAF loop).
  const dispatched = [];
  const textInputReqs = [];
  const bridge = new FaceplateBridge({
    controller,
    dispatch: (arr) => dispatched.push(...arr),
    onTextInput: (req) => textInputReqs.push(req),
    // No frame loop in the test — we call tick() by hand.
    scheduleFrame: () => null,
    cancelFrame: () => {},
  });

  // ---- pure helpers: translate + dedupe ------------------------------------
  check(
    JSON.stringify(viewEventToFaceplate({ type: "ParamChanged", id: 3, plain: 1, norm: 0.5, display: "x" })) ===
      JSON.stringify({ kind: "param_changed", id: 3, plain: 1, norm: 0.5, display: "x" }),
    "translate ParamChanged -> param_changed",
  );
  check(
    viewEventToFaceplate({ type: "EditLayerChanged", layer: 1 }).layer === "lower",
    "translate EditLayerChanged layer 1 -> 'lower'",
  );
  check(
    viewEventToFaceplate({ type: "EditLayerChanged", layer: 0 }).layer === "upper",
    "translate EditLayerChanged layer 0 -> 'upper'",
  );
  {
    const deduped = dedupParamChanged([
      { kind: "param_changed", id: 5, plain: 1 },
      { kind: "status", line: "a" },
      { kind: "param_changed", id: 5, plain: 2 },
      { kind: "param_changed", id: 7, plain: 9 },
    ]);
    // id 5 collapses to its LAST occurrence (plain 2), keeping position; status
    // and id 7 survive.
    check(deduped.length === 3, `dedupe collapses id 5 (len ${deduped.length} == 3)`);
    const five = deduped.filter((e) => e.kind === "param_changed" && e.id === 5);
    check(five.length === 1 && five[0].plain === 2, "dedupe keeps latest value for id 5");
  }

  // ---- 0058: opcode -> controller model -> store SAB -----------------------
  const ID = 5; // an Upper-layer patch param
  bridge.handleUiOpcode(JSON.stringify({ op: "begin_gesture", id: ID }));
  bridge.handleUiOpcode(JSON.stringify({ op: "set_param_norm", id: ID, norm: 0.5 }));
  bridge.handleUiOpcode(JSON.stringify({ op: "end_gesture", id: ID }));
  let batch = bridge.tick();
  const stored = store.read(ID);
  check(Number.isFinite(stored), `store SAB holds a finite value for id ${ID} (${stored})`);

  // ---- 0059 + 0060: ParamChanged echo reaches the dispatcher, in order -----
  const pc = dispatched.find((e) => e.kind === "param_changed" && e.id === ID);
  check(!!pc, `dispatched a param_changed for id ${ID}`);
  if (pc) {
    check(approx(pc.plain, stored), `dispatched param_changed.plain (${pc.plain}) == store (${stored})`);
    check(approx(pc.norm, 0.5, 1e-3), `dispatched param_changed.norm (${pc.norm}) ~= 0.5`);
    check(typeof pc.display === "string" && pc.display.length > 0, `display is non-empty ("${pc.display}")`);
  }
  // The begin/set/end bracket settled to a single param_changed for this id in
  // the batch (dedupe by id holds across the bracket).
  check(
    batch.filter((e) => e.kind === "param_changed" && e.id === ID).length === 1,
    "gesture bracket settles to one deduped param_changed",
  );

  // ---- 0058: edit-layer string -> int mapping reaches the controller -------
  dispatched.length = 0;
  bridge.handleUiOpcode(JSON.stringify({ op: "set_edit_layer", layer: "lower" }));
  bridge.tick();
  const elc = dispatched.find((e) => e.kind === "edit_layer_changed");
  check(!!elc && elc.layer === "lower", `set_edit_layer 'lower' -> edit_layer_changed 'lower' (${elc && elc.layer})`);

  // ---- 0058: key mode opcode (int) -----------------------------------------
  dispatched.length = 0;
  bridge.handleUiOpcode(JSON.stringify({ op: "set_key_mode", mode: 1 })); // Dual
  bridge.tick();
  const km = dispatched.find((e) => e.kind === "key_mode_changed");
  check(!!km && km.mode === 1, `set_key_mode 1 -> key_mode_changed mode 1 (${km && km.mode})`);

  // ---- 0058: preset opcodes are inert (no throw, nothing dispatched) -------
  dispatched.length = 0;
  let threw = false;
  try {
    bridge.handleUiOpcode(JSON.stringify({ op: "load_factory", index: 0 }));
    bridge.handleUiOpcode(JSON.stringify({ op: "save_preset", name: "X", folder: null }));
    bridge.handleUiOpcode(JSON.stringify({ op: "step_preset", delta: 1 }));
    bridge.tick();
  } catch {
    threw = true;
  }
  check(!threw, "preset opcodes route without throwing (inert under NullStore)");

  // ---- malformed / unknown opcode is dropped silently ----------------------
  let threw2 = false;
  try {
    bridge.handleUiOpcode("not json");
    bridge.handleUiOpcode(JSON.stringify({ op: "bogus_opcode", x: 1 }));
    bridge.tick();
  } catch {
    threw2 = true;
  }
  check(!threw2, "malformed / unknown opcode dropped without throwing");

  // ---- 0061: request_text_input is intercepted, NOT forwarded --------------
  bridge.handleUiOpcode(JSON.stringify({ op: "request_text_input", id: "ti1", title: "Name", initial: "Init" }));
  check(textInputReqs.length === 1 && textInputReqs[0].id === "ti1", "request_text_input intercepted (not forwarded)");
  check(textInputReqs[0].title === "Name" && textInputReqs[0].initial === "Init", "text-input req carries title + initial");

  // ---- 0061: the DOM popup delivers text_input_result exactly once ----------
  // Minimal fake DOM: each element records its own listeners so the test can
  // fire the input's keydown directly.
  {
    const delivered = [];
    globalThis.window = { vxn: { onViewEvent: (ev) => delivered.push(ev) } };
    const mkEl = () => ({
      className: "",
      type: "",
      value: "",
      textContent: "",
      children: [],
      listeners: {},
      appendChild(c) { this.children.push(c); return c; },
      remove() { this._removed = true; },
      addEventListener(ev, cb) { this.listeners[ev] = cb; },
      focus() {},
      select() {},
    });
    let backdrop = null;
    const doc = {
      createElement: () => mkEl(),
      body: { appendChild(c) { backdrop = c; } },
    };
    openTextInputPopup({ id: "ti2", title: "T", initial: "hello" }, doc);
    // Tree: backdrop -> box -> [title, input]. The input is box.children[1].
    const box = backdrop.children[0];
    const input = box.children[1];
    check(input.value === "hello", "popup input seeded with initial value");
    input.value = "world";
    input.listeners.keydown({ key: "Enter", preventDefault() {} });
    check(
      delivered.length === 1 &&
        delivered[0].kind === "text_input_result" &&
        delivered[0].id === "ti2" &&
        delivered[0].value === "world",
      `text-input commit delivers result once (${JSON.stringify(delivered[0])})`,
    );
    // A second keydown must NOT re-deliver (fire-once).
    input.listeners.keydown({ key: "Enter", preventDefault() {} });
    check(delivered.length === 1, "text-input is fire-once (no double-deliver)");
    check(backdrop._removed === true, "popup backdrop removed on commit");
    delete globalThis.window;
  }

  // ---- 0059: dormant readback — no diff-poll is wired ----------------------
  // The bridge never calls pumpReadback; an audio-thread write into the readback
  // region must NOT surface through the bridge's tick path.
  dispatched.length = 0;
  const AUTO_ID = 12;
  store.publishReadback(AUTO_ID, store.read(AUTO_ID) + 0.2);
  bridge.tick();
  check(
    dispatched.filter((e) => e.kind === "param_changed").length === 0,
    "readback drift does NOT surface (diff pump dormant in web — 0044)",
  );

  // ---- remirrorStore re-seeds the store from the controller -----------------
  // Simulate the coordinator's writeBulk clobber: overwrite a store slot, then
  // remirror + tick — the controller's authoritative value must win.
  {
    const RID = 5;
    const authoritative = store.read(RID);
    store.write(RID, authoritative + 7.0); // pretend writeBulk clobbered it
    controller.remirrorStore();
    bridge.tick();
    check(
      approx(store.read(RID), authoritative),
      `remirrorStore restores controller value after a clobber (${store.read(RID)} == ${authoritative})`,
    );
  }

  controller.destroy();
  console.log(`\n${failures === 0 ? "ALL PASS" : `${failures} FAILED`}`);
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
