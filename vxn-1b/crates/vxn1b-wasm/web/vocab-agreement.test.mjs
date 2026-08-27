// Cross-language vocabulary agreement (0316).
//
// The page's custom ops carry NAMES — "upper"/"lower", "source"/"dest"/
// "curve"/"scale", "off"/"upper"/"lower" — and three places used to decide
// independently what each one meant: `vxn1b-ui-web` (strings → enums, native
// editor), `vxn1b-web-controller` (ordinals → enums, browser), and
// `faceplate-bridge.mjs` (strings → ordinals, browser). The first two now read
// one table, `vxn1b_engine::vocab`. This file pins the third to it.
//
// Why it matters more than it looks: every failure here is SILENT. A renamed
// field name makes `MATRIX_FIELD[msg.field]` `undefined` and `routeOpcode`
// returns false — the knob moves on screen and the sound does not change. A
// reordered ordinal is worse: the op lands, on the wrong field.
//
// Read out of the BUILT artifact, and FAILS rather than skips if it is missing
// (0295).
//
//   cargo build -p vxn1b-web-controller --target wasm32-unknown-unknown --release
//   node --test vxn-1b/crates/vxn1b-wasm/web/vocab-agreement.test.mjs

import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { LAYER, MATRIX_FIELD, SCOPE_TAP, SPLIT_POINT } from "./faceplate-bridge.mjs";
import {
  VE_PARAM_CHANGED,
  VE_MATRIX_SNAPSHOT,
  VE_KEY_STATE,
  VE_PRESET_LOADED,
  VE_CORPUS_CHANGED,
  VE_STATUS,
  PRESET_SRC_NONE,
  PRESET_SRC_FACTORY,
  PRESET_SRC_USER,
  JW_PUT,
  JW_DELETE,
  JW_PUT_FOLDER,
  JW_DELETE_FOLDER,
} from "./controller.mjs";
import {
  LAYER_COUNT,
  LAYER_L1,
  LAYER_L2,
  MATRIX_SLOTS,
  MATRIX_FIELD_SOURCE,
  MATRIX_FIELD_DEST,
  MATRIX_FIELD_CURVE,
  MATRIX_FIELD_SCALE_SRC,
} from "./event-codec.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const WASM = path.resolve(
  here,
  "../../../../target/wasm32-unknown-unknown/release/vxn1b_web_controller.wasm",
);

let wasmBytes = null;
try {
  wasmBytes = await readFile(WASM);
} catch {
  throw new Error(
    `controller wasm not built at ${WASM}\n` +
      "run: cargo build -p vxn1b-web-controller --target wasm32-unknown-unknown --release",
  );
}

/// The vocabulary as the shipped Rust defines it.
async function rustVocab() {
  const { instance } = await WebAssembly.instantiate(wasmBytes, {});
  const x = instance.exports;
  x.vxnc_new();
  const bytes = new Uint8Array(
    x.memory.buffer,
    x.vxnc_vocab_json_ptr(),
    x.vxnc_vocab_json_len(),
  );
  const json = JSON.parse(new TextDecoder().decode(bytes));
  x.vxnc_destroy();
  return json;
}

const HINT =
  "the page's table and vxn1b_engine::vocab disagree — a dropped opcode or, " +
  "worse, an op landing on the wrong field. Fix the JS to match the Rust.";

test("the bridge's layer names match the engine's", async () => {
  const v = await rustVocab();
  assert.deepEqual(LAYER, v.layer, HINT);
  // ...and the codec's own layer ids are the same ordinals.
  assert.equal(LAYER_L1, v.layer.upper);
  assert.equal(LAYER_L2, v.layer.lower);
  assert.equal(LAYER_COUNT, v.layerCount);
});

test("the bridge's matrix-field names and ordinals match the engine's", async () => {
  const v = await rustVocab();
  assert.deepEqual(MATRIX_FIELD, v.matrixField, HINT);
  assert.deepEqual(
    {
      source: MATRIX_FIELD_SOURCE,
      dest: MATRIX_FIELD_DEST,
      curve: MATRIX_FIELD_CURVE,
      scale: MATRIX_FIELD_SCALE_SRC,
    },
    v.matrixField,
    "event-codec.mjs's field constants must agree too — the ring packs them",
  );
});

test("the bridge's scope taps match the engine's ScopeTap codes", async () => {
  const v = await rustVocab();
  assert.deepEqual(SCOPE_TAP, v.scopeTap, HINT);
});

test("the matrix geometry and split range are the engine's, not the page's", async () => {
  const v = await rustVocab();
  assert.equal(MATRIX_SLOTS, v.matrixSlots);
  assert.deepEqual(SPLIT_POINT, v.splitPoint, "split slider range/default");
});

// A vocabulary the page can send but Rust cannot decode is a dead opcode; one
// Rust accepts but the page never sends is a decoder with no producer. Neither
// is caught by the equality checks above if BOTH sides gain the same typo, so
// assert the key sets against the names actually used in `routeOpcode`.
test("every name the page can send is one the engine decodes", async () => {
  const v = await rustVocab();
  const src = await readFile(path.join(here, "faceplate-bridge.mjs"), "utf8");
  for (const [table, names] of [
    ["layer", Object.keys(v.layer)],
    ["matrixField", Object.keys(v.matrixField)],
    ["scopeTap", Object.keys(v.scopeTap)],
  ]) {
    assert.ok(names.length > 0, `${table} is empty`);
    for (const n of names) {
      assert.ok(src.includes(`${n}:`), `${table} name "${n}" appears in no page table`);
    }
  }
});

// The drain's own tags. A wrong VE_* number does not fail loudly either: the
// decoder falls through its switch and the record is skipped, so the page just
// stops repainting whatever that record carried.
test("the ViewEvent, PresetSource and journal tags match the controller's", async () => {
  const v = await rustVocab();
  assert.deepEqual(
    {
      paramChanged: VE_PARAM_CHANGED,
      matrixSnapshot: VE_MATRIX_SNAPSHOT,
      keyState: VE_KEY_STATE,
      presetLoaded: VE_PRESET_LOADED,
      corpusChanged: VE_CORPUS_CHANGED,
      status: VE_STATUS,
    },
    v.viewEvent,
    HINT,
  );
  assert.deepEqual(
    { none: PRESET_SRC_NONE, factory: PRESET_SRC_FACTORY, user: PRESET_SRC_USER },
    v.presetSource,
    HINT,
  );
  assert.deepEqual(
    { put: JW_PUT, delete: JW_DELETE, putFolder: JW_PUT_FOLDER, deleteFolder: JW_DELETE_FOLDER },
    v.journal,
    HINT,
  );
});
