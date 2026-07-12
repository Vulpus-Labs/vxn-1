// End-to-end factory-preset test (ticket 0159, minimal) over the REAL controller
// wasm + baked factory.bin. Run after `cargo xtask web`:
//   node --test crates/vxn2-wasm/web/controller-wasm.test.mjs
//
// Loads the actual `vxn2_web_controller.wasm` and `factory.bin` from the built
// bundle, drives the WebController through loadFactoryAsset → corpusJson →
// loadFactory(0) → tick, and asserts the factory bank + a PresetLoaded surface.
// Skips (not fails) when the bundle isn't built.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { WebController } from "./controller.mjs";

const DIST = new URL("../../../../target/web-dist/", import.meta.url);
const WASM = fileURLToPath(new URL("vxn2_web_controller.wasm", DIST));
const FACTORY = fileURLToPath(new URL("factory.bin", DIST));
const HAVE = existsSync(WASM) && existsSync(FACTORY);

test("real controller wasm loads the factory bank and a preset", { skip: !HAVE }, async () => {
  const ctrl = new WebController({ wasmBytes: readFileSync(WASM) });
  await ctrl.instantiate();

  // Load the baked bank.
  const count = ctrl.loadFactoryAsset(readFileSync(FACTORY));
  assert.ok(count >= 5, `expected the factory bank, got ${count}`);

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
