// End-to-end factory-preset test (ticket 0159) over the REAL controller wasm +
// the REAL baked factory.bin:
//   cargo run -p vxn2-xtask -- web
//   node --test vxn-2/crates/vxn2-wasm/web/controller-wasm.test.mjs
//
// Drives the WebController through loadFactoryAsset → corpusJson → loadFactory(0)
// → tick, and asserts the shipped bank plus a PresetLoaded surface.
//
// This is the one test here that genuinely needs the BUNDLE rather than a crate
// artifact, because it asserts against the real baked bank. It therefore still
// reads `target/web-dist/` — which both ports' `xtask web` create and wipe — so
// a run right after vxn-1's build will fail rather than pass. That is the
// intended behaviour: it FAILS and says what to run (ticket 0295). It used to
// skip, which is how ticket 0285 stayed hidden for weeks.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { WebController } from "./controller.mjs";

const DIST = new URL("../../../../target/web-dist/", import.meta.url);
const WASM = fileURLToPath(new URL("vxn2_web_controller.wasm", DIST));
const FACTORY = fileURLToPath(new URL("factory.bin", DIST));

const BUILD_HINT = "cargo run -p vxn2-xtask -- web";

test("real controller wasm loads the factory bank and a preset", async () => {
  assert.ok(
    existsSync(WASM) && existsSync(FACTORY),
    `the vxn-2 web bundle is not built at ${fileURLToPath(DIST)} — this test ` +
      `must not be skipped. Build it:\n  ${BUILD_HINT}`,
  );
  const ctrl = new WebController({ wasmBytes: readFileSync(WASM) });
  await ctrl.instantiate();

  // Load the baked bank.
  const count = ctrl.loadFactoryAsset(readFileSync(FACTORY));
  assert.ok(
    count >= 5,
    `expected the vxn-2 factory bank, got ${count} presets. A low or zero count ` +
      `usually means target/web-dist/ holds ANOTHER product's bundle — both ports' ` +
      `xtask web share that directory. Rebuild vxn-2\u2019s:\n  ${BUILD_HINT}`,
  );

  // The corpus JSON lists factory presets.
  const corpus = ctrl.corpusJson();
  assert.ok(Array.isArray(corpus.factory) && corpus.factory.length > 0, "empty factory corpus");

  // Load preset 0 → next tick surfaces PresetLoaded + a param re-broadcast.
  ctrl.tick(); // clear boot seed
  ctrl.loadFactory(0);
  const events = ctrl.tick();
  const loaded = events.find((e) => e.kind === "preset_loaded");
  assert.ok(loaded, "no preset_loaded event after loadFactory(0)");
  assert.equal(loaded.source && loaded.source.kind, "factory");
  assert.equal(loaded.source.index, 0);
  assert.ok(
    events.some((e) => e.kind === "param_changed"),
    "factory load did not re-broadcast params",
  );

  ctrl.destroy();
});
