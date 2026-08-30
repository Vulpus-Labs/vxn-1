// Headless test for the controller wasm glue (0291), driven against the REAL
// `vxn1b-web-controller` module rather than a hand-rolled byte fixture — so the
// Rust packer and this decoder are pinned to each other, not to a copy of the
// format that can rot in step with a bug.
//
//   cargo build -p vxn1b-web-controller --target wasm32-unknown-unknown --release
//   node --test vxn-1b/crates/vxn1b-wasm/web/controller.test.mjs
//
// FAILS (it does not skip) if the wasm has not been built — the posture 0295
// settled for vxn-2: a missing artefact must never read as a pass.

import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { WebController, decodeViewEvents } from "./controller.mjs";
import { ParamStore, createParamSAB, TOTAL_PARAMS, patchClapId } from "./param-store.mjs";
import {
  LAYER_L1,
  LAYER_L2,
  MATRIX_FIELD_DEST,
  MATRIX_FIELD_SOURCE,
  MATRIX_FIELD_POLARITY,
  MATRIX_FIELD_SCALE_SRC,
  MATRIX_FIELD_SHAPE,
  MATRIX_FIELD_SCALE_SHAPE,
  MATRIX_FIELD_ENABLED,
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

const fresh = async (withStore = false) => {
  const store = withStore ? new ParamStore(createParamSAB()) : null;
  const c = new WebController({ wasmBytes, store });
  await c.instantiate();
  return c;
};

// Cutoff is PATCH_PARAMS index 19 (the Osc/mixer block is the first 19) and is
// `Exp { mid: 800 }` over ~16 Hz..16 kHz — the tapered, Hz-displaying param the
// norm assertions need. `assertIsCutoff` below makes a table reorder fail with a
// readable message instead of silently testing some other control.
const CUTOFF_PATCH_INDEX = 19;
const cutoffL1 = () => patchClapId(LAYER_L1, CUTOFF_PATCH_INDEX);
const cutoffL2 = () => patchClapId(LAYER_L2, CUTOFF_PATCH_INDEX);
const assertIsCutoff = (ev) =>
  assert.match(
    ev.display,
    /Hz/i,
    `expected the Cutoff param at patch index ${CUTOFF_PATCH_INDEX}, got display "${ev.display}" — has PATCH_PARAMS been reordered?`,
  );

test("instantiate agrees with the engine's param layout", async () => {
  const c = await fresh();
  assert.equal(c.totalParams, TOTAL_PARAMS);
  assert.equal(c.totalParams, 185);
  assert.equal(c.patchCount, 75);
  assert.equal(c.patchCount * 2 + 35, TOTAL_PARAMS);
  c.destroy();
});

test("every vxnc_ui_* export has a wrapper method", async () => {
  const c = await fresh();
  // Guards against an opcode being added Rust-side and silently going unwired.
  const wrapped = new Set([
    "vxnc_ui_set_param", "vxnc_ui_set_param_norm", "vxnc_ui_begin_gesture",
    "vxnc_ui_end_gesture", "vxnc_ui_editor_ready", "vxnc_ui_set_key_mode",
    "vxnc_ui_set_split_point", "vxnc_ui_set_lfo2_link", "vxnc_ui_set_matrix",
    "vxnc_ui_copy_layer", "vxnc_ui_reset_layer",
    "vxnc_ui_load_factory", "vxnc_ui_step_preset",
    "vxnc_ui_save_preset", "vxnc_ui_load_user", "vxnc_ui_rename_preset",
    "vxnc_ui_delete_preset", "vxnc_ui_move_preset", "vxnc_ui_new_folder",
    "vxnc_ui_rename_folder", "vxnc_ui_delete_folder",
  ]);
  const exported = Object.keys(c.x).filter((k) => k.startsWith("vxnc_ui_"));
  const missing = exported.filter((k) => !wrapped.has(k));
  assert.deepEqual(missing, [], `unwrapped controller opcodes: ${missing.join(", ")}`);
  // …and nothing in the list has been removed Rust-side.
  const gone = [...wrapped].filter((k) => typeof c.x[k] !== "function");
  assert.deepEqual(gone, [], `wrapper references missing exports: ${gone.join(", ")}`);
  c.destroy();
});

test("the first tick seeds the matrix and key echoes, then quiesces", async () => {
  const c = await fresh();
  const first = c.tick();
  const kinds = first.map((e) => e.kind);
  assert.ok(kinds.includes("matrix"), `no matrix seed: ${kinds}`);
  assert.ok(kinds.includes("keys"), `no key seed: ${kinds}`);
  assert.deepEqual(c.tick(), [], "second tick should be silent");
  c.destroy();
});

test("a param write decodes to the page's param_changed shape", async () => {
  const c = await fresh();
  c.tick();
  const id = cutoffL1();
  c.setParam(id, 900);
  const evs = c.tick();
  const p = evs.find((e) => e.kind === "param_changed");
  assert.ok(p, "no param_changed");
  assert.equal(p.id, id);
  assert.ok(Math.abs(p.plain - 900) < 1);
  assert.ok(p.norm > 0 && p.norm < 1, `norm out of range: ${p.norm}`);
  assert.equal(typeof p.display, "string");
  assert.notEqual(p.display, "", "display must be descriptor-derived, not empty");
  assert.notEqual(p.display, String(p.plain), "display must not be a raw stringify");
  assertIsCutoff(p);
  // Exactly one record for one write: echo on, no double-emit.
  assert.equal(evs.filter((e) => e.kind === "param_changed").length, 1);
  c.destroy();
});

test("the norm path carries the descriptor taper, not a linear position", async () => {
  const c = await fresh();
  c.tick();
  const id = cutoffL1();
  c.setParamNorm(id, 0.5);
  const p = c.tick().find((e) => e.kind === "param_changed");
  assert.ok(p);
  assert.ok(Math.abs(p.norm - 0.5) < 1e-3, `norm did not round-trip: ${p.norm}`);
  assertIsCutoff(p);
  // Exp{mid:800}: the fader midpoint reads ~800 Hz. A linear map would put it
  // near 8 kHz, so this fails if the taper is ever dropped from the norm path.
  assert.ok(p.plain < 4000, `norm path looks linear: ${p.plain}`);
  c.destroy();
});

test("the matrix record carries both layers, 16 slots, topology only", async () => {
  const c = await fresh();
  c.tick();
  c.setMatrix(LAYER_L2, 3, MATRIX_FIELD_DEST, 5);
  const m = c.tick().find((e) => e.kind === "matrix");
  assert.ok(m, "no matrix echo after a topology edit");
  assert.equal(m.slots.length, 2);
  assert.equal(m.slots[0].length, 16);
  assert.equal(m.slots[1].length, 16);
  assert.equal(m.slots[1][3].dest, 5);
  // Depth is a param and must NOT be in the topology record. The other six are
  // all here: an under-read of this record does not fail locally, it shifts the
  // cursor and corrupts every LATER record in the drain (the `unknown ViewEvent
  // tag` cascade), so the width is pinned by naming every key.
  assert.deepEqual(Object.keys(m.slots[0][0]).sort(), [
    "dest",
    "enabled",
    "polarity",
    "scale",
    "scaleShape",
    "shape",
    "source",
  ]);
  // `enabled` is 0/1 on the wire and a bool once decoded, matching the native
  // `slots_json` the same panel reads.
  assert.equal(typeof m.slots[0][0].enabled, "boolean");
  c.destroy();
});

// The ordinals and the snapshot byte order are two DIFFERENT orderings of the
// same seven fields — `scale` is ordinal 3 but the fifth byte packed — so a
// decoder that reads them in ordinal order still parses a full-width record and
// merely reports the wrong values. Distinct values per field, chosen so no two
// adjacent fields share one, turn any transposition into a failure.
test("every matrix field round-trips through its own ordinal and its own byte", async () => {
  const c = await fresh();
  // The default patch is not an empty table, so the untouched-slot check below
  // compares against what was actually there, not against zeros.
  const before = c.tick().find((e) => e.kind === "matrix");
  assert.ok(before, "no matrix seed on the first tick");
  const untouched = { ...before.slots[1][2] };
  const want = {
    source: 1,
    dest: 3,
    polarity: 2,
    shape: 1,
    scale: 4,
    scaleShape: 2,
    enabled: true,
  };
  c.setMatrix(LAYER_L1, 2, MATRIX_FIELD_SOURCE, want.source);
  c.setMatrix(LAYER_L1, 2, MATRIX_FIELD_DEST, want.dest);
  c.setMatrix(LAYER_L1, 2, MATRIX_FIELD_POLARITY, want.polarity);
  c.setMatrix(LAYER_L1, 2, MATRIX_FIELD_SHAPE, want.shape);
  c.setMatrix(LAYER_L1, 2, MATRIX_FIELD_SCALE_SRC, want.scale);
  c.setMatrix(LAYER_L1, 2, MATRIX_FIELD_SCALE_SHAPE, want.scaleShape);
  c.setMatrix(LAYER_L1, 2, MATRIX_FIELD_ENABLED, 1);
  const m = c.tick().find((e) => e.kind === "matrix");
  assert.ok(m, "no matrix echo after the edits");
  assert.deepEqual(m.slots[0][2], want);
  // The edits landed on layer 1 slot 2 and nowhere else — the same slot index
  // on the other layer is untouched, which a layer-shifted decode would miss.
  assert.deepEqual(m.slots[1][2], untouched);
  c.destroy();
});

test("the key record carries mode, split and the lfo2 link as a bool", async () => {
  const c = await fresh();
  c.tick();
  c.setKeyMode(2); // Split
  c.setSplitPoint(48);
  c.setLfo2Link(true);
  const k = c.tick().find((e) => e.kind === "keys");
  assert.ok(k, "no key echo");
  assert.equal(k.mode, 2);
  assert.equal(k.split, 48);
  assert.equal(k.link, true, "link must decode to a boolean, matching the native echo");
  c.destroy();
});

test("a factory load decodes preset_loaded with a nested source and re-broadcasts", async () => {
  const c = await fresh();
  assert.ok(c.factoryLen() > 0, "embedded factory bank is empty");
  c.tick();
  c.loadFactory(0);
  const evs = c.tick();
  const loaded = evs.find((e) => e.kind === "preset_loaded");
  assert.ok(loaded, "no preset_loaded");
  assert.equal(typeof loaded.name, "string");
  assert.ok(loaded.name.length > 0);
  // The page reads `source` as an object, not the flat u32 the wire carries.
  assert.deepEqual(loaded.source, { kind: "factory", index: 0 });
  assert.ok(Array.isArray(loaded.warnings));
  const ids = new Set(evs.filter((e) => e.kind === "param_changed").map((e) => e.id));
  assert.equal(ids.size, TOTAL_PARAMS, "a preset load must re-broadcast the table");
  c.destroy();
});

test("the corpus is readable before any tick — no factory fetch", async () => {
  const c = await fresh();
  const corpus = c.corpusJson();
  assert.ok(Array.isArray(corpus.factory));
  assert.ok(corpus.factory.length > 0, "embedded bank missing from the corpus");
  assert.ok(Array.isArray(corpus.user));
  c.destroy();
});

test("string args round-trip, including the root-folder sentinel and non-ASCII", async () => {
  const c = await fresh();
  c.tick();
  // Root folder (null → ARG_NONE) and a non-ASCII display name.
  c.savePreset("Pâté Bass", null);
  const evs = c.tick();
  assert.ok(evs.some((e) => e.kind === "preset_corpus_changed"), "save did not announce");
  const corpus = c.corpusJson();
  const names = JSON.stringify(corpus);
  assert.ok(names.includes("Pâté Bass"), `non-ASCII name lost: ${names}`);

  const ops = c.takeJournal();
  const put = ops.find((o) => o.kind === "put");
  assert.ok(put, "no Put journalled");
  assert.ok(put.key.includes("Bass"), `unexpected key ${put.key}`);
  assert.ok(put.bytes instanceof Uint8Array && put.bytes.length > 0);
  assert.deepEqual(c.takeJournal(), [], "journal did not drain");
  c.destroy();
});

test("saving into a folder journals the folder op too", async () => {
  const c = await fresh();
  c.tick();
  c.savePreset("Lead", "Leads");
  c.tick();
  const ops = c.takeJournal();
  // Folder ops carry `name`, preset ops carry `key` — preset-storage.mjs's
  // contract, which applyWrites switches on.
  assert.ok(ops.some((o) => o.kind === "put_folder" && o.name === "Leads"));
  assert.ok(ops.some((o) => o.kind === "put" && o.key === "Leads/Lead.toml"));
  c.destroy();
});

test("state snapshot/restore and TOML export/import round-trip", async () => {
  const c = await fresh();
  c.tick();
  const id = cutoffL1();
  c.setParam(id, 640);
  c.tick();

  const blob = c.snapshotState();
  assert.ok(blob.length > 0);
  const toml = c.exportToml("Shared");
  assert.ok(toml.includes("Shared"), "export lost the name");

  c.setParam(id, 120);
  c.tick();
  assert.equal(c.restoreState(blob), true);
  let p = c.tick().find((e) => e.kind === "param_changed" && e.id === id);
  assert.ok(p && Math.abs(p.plain - 640) < 1, "restore did not reinstate the value");

  c.setParam(id, 120);
  c.tick();
  assert.equal(c.importToml(toml), true);
  p = c.tick().find((e) => e.kind === "param_changed" && e.id === id);
  assert.ok(p && Math.abs(p.plain - 640) < 1, "import did not reinstate the value");
  c.destroy();
});

test("malformed state and TOML are rejected without mutating", async () => {
  const c = await fresh();
  c.tick();
  assert.equal(c.restoreState(new Uint8Array([1, 2, 3, 4])), false);
  assert.equal(c.importToml("not a preset at all"), false);
  assert.deepEqual(c.tick(), [], "a rejected load still emitted events");
  c.destroy();
});

test("hydration replays a stored record without journalling", async () => {
  const a = await fresh();
  a.tick();
  a.savePreset("Hydrated", "F");
  a.tick();
  const put = a.takeJournal().find((o) => o.kind === "put" && o.key === "F/Hydrated.toml");
  assert.ok(put, "no Put to hydrate from");

  const b = await fresh();
  b.hydrateFolder("F");
  assert.equal(b.hydratePreset("F/Hydrated.toml", put.bytes), 1);
  b.hydrateDone();
  assert.deepEqual(b.takeJournal(), [], "hydration must not journal");
  assert.ok(JSON.stringify(b.corpusJson()).includes("Hydrated"));
  a.destroy();
  b.destroy();
});

test("mirrorToStore writes only changed slots and seeds the whole table first", async () => {
  const c = await fresh(true);
  c.tick();
  // First mirror writes every slot (NaN seed).
  assert.equal(c.mirrorToStore(), TOTAL_PARAMS);
  // Nothing moved → nothing written.
  assert.equal(c.mirrorToStore(), 0);

  const id = cutoffL1();
  c.setParam(id, 777);
  c.tick();
  assert.equal(c.mirrorToStore(), 1, "one edit should write exactly one slot");
  assert.ok(Math.abs(c.store.read(id) - 777) < 1, "the SAB did not receive the edit");
  c.destroy();
});

test("copy_layer moves the patch across layers through the mirror", async () => {
  const c = await fresh(true);
  c.tick();
  c.mirrorToStore();
  const l1 = cutoffL1();
  const l2 = cutoffL2();
  c.setParam(l1, 950);
  c.setMatrix(LAYER_L1, 5, MATRIX_FIELD_SOURCE, 3);
  c.tick();
  c.mirrorToStore();

  c.copyLayer(0, 1);
  const evs = c.tick();
  const written = c.mirrorToStore();
  assert.ok(written > 0, "copy_layer wrote nothing to the SAB");
  assert.ok(Math.abs(c.store.read(l2) - 950) < 1, "the copy did not reach layer 2");
  // Topology follows too, and it arrives as a matrix record (the bridge's
  // resend trigger) rather than through the param mirror.
  const m = evs.find((e) => e.kind === "matrix");
  assert.ok(m, "copy_layer produced no matrix echo");
  assert.equal(m.slots[1][5].source, 3);
  c.destroy();
});

test("an unknown record tag fails loudly rather than emitting garbage", () => {
  // count=1, tag=99
  const buf = new ArrayBuffer(8);
  const dv = new DataView(buf);
  dv.setUint32(0, 1, true);
  dv.setUint32(4, 99, true);
  assert.throws(() => decodeViewEvents(buf, 0, 8), /unknown ViewEvent tag 99/);
});
