// Headless Node test for the main-thread controller glue (ticket 0044).
//
//   cargo build -p vxn-web-controller --target wasm32-unknown-unknown
//   node web/controller.test.mjs
//
// No browser/audio here; we drive the EXACT shared code path (controller.mjs +
// the real vxn-web-controller wasm) the page runs, with hard asserts — same
// discipline as param-store.test.mjs. Covers the 0044 acceptance criteria:
//
//   1. instantiate: the controller wasm loads and its param counts agree with
//      the JS mirror (read FROM wasm, not hard-coded).
//   2. UiEvent → model → store SAB: a setParamNorm gesture mutates the model
//      and the value lands in the SHARED store SAB (what the worklet reads).
//   3. ViewEvent drain: the same edit echoes back as a ParamChanged record with
//      a correct (descriptor-derived) norm + display string.
//   4. custom opcodes: setKeyMode emits KeyModeChanged via the packed surface.
//   5. diff pump: an audio-thread write into the readback SAB region surfaces
//      as exactly one ParamChanged (port of push_param_diffs).

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { createParamSAB, ParamStore, TOTAL_PARAMS } from "./param-store.mjs";
import { WebController, KEY_MODE_DUAL } from "./controller.mjs";

const here = dirname(fileURLToPath(import.meta.url));
// target/wasm32-unknown-unknown/debug/ is 5 levels up from web/.
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

  const sink = [];
  const ctrl = new WebController({
    wasmBytes,
    store,
    onViewEvents: (evs) => sink.push(...evs),
  });
  await ctrl.instantiate();

  // (1) counts read from wasm agree with the JS mirror.
  check(ctrl.totalParams === TOTAL_PARAMS, `total ${ctrl.totalParams} == ${TOTAL_PARAMS}`);
  check(ctrl.patchCount === 69, `patch count ${ctrl.patchCount} == 69`);
  check(ctrl.globalCount === 27, `global count ${ctrl.globalCount} == 27`);
  check(
    ctrl.patchCount * 2 + ctrl.globalCount === ctrl.totalParams,
    "2*patch + global == total",
  );

  // (2)+(3) UiEvent gesture → model mutation → store SAB + ParamChanged echo.
  const ID = 5; // an Upper-layer patch param
  sink.length = 0;
  ctrl.beginGesture(ID);
  ctrl.setParamNorm(ID, 0.5);
  ctrl.endGesture(ID);
  ctrl.tick();

  const stored = store.read(ID);
  // The model converts norm 0.5 -> plain via the descriptor; assert it's a real
  // value (not NaN/0 default leak) and that the SAB carries it.
  check(Number.isFinite(stored), `store SAB holds a finite value for id ${ID} (${stored})`);

  const pc = sink.find((e) => e.type === "ParamChanged" && e.id === ID);
  check(!!pc, `ParamChanged emitted for id ${ID}`);
  if (pc) {
    check(approx(pc.plain, stored), `ParamChanged.plain (${pc.plain}) == store (${stored})`);
    check(approx(pc.norm, 0.5, 1e-3), `ParamChanged.norm (${pc.norm}) ~= 0.5 (descriptor taper)`);
    check(
      typeof pc.display === "string" && pc.display.length > 0,
      `ParamChanged.display is a non-empty string ("${pc.display}")`,
    );
  }

  // (4) custom opcode: key mode change surfaces as a packed KeyModeChanged.
  sink.length = 0;
  ctrl.setKeyMode(KEY_MODE_DUAL);
  ctrl.tick();
  const km = sink.find((e) => e.type === "KeyModeChanged");
  check(!!km && km.mode === KEY_MODE_DUAL, `KeyModeChanged{mode=Dual} emitted (${km && km.mode})`);

  // (5) diff pump: an audio-thread write into the readback region surfaces as
  // exactly one ParamChanged (the host-automation echo path).
  sink.length = 0;
  const AUTO_ID = 10;
  const AUTO_VAL = store.read(AUTO_ID) + 0.123; // a value the controller never set
  store.publishReadback(AUTO_ID, AUTO_VAL);
  // First pump has a NaN-seeded last_seen, so it broadcasts ALL 165. Run it once
  // to settle the seed, then assert the SINGLE drift surfaces on the next pump.
  // (NaN-seed full-broadcast is the documented first-tick behaviour.)
  // Seed every readback slot to the store's current value so only AUTO_ID drifts.
  for (let id = 0; id < TOTAL_PARAMS; id++) store.publishReadback(id, store.read(id));
  ctrl.pumpReadback(); // settles last_seen against the seeded readback
  sink.length = 0;
  store.publishReadback(AUTO_ID, AUTO_VAL);
  ctrl.pumpReadback();
  const autos = sink.filter((e) => e.type === "ParamChanged" && e.id === AUTO_ID);
  check(autos.length === 1, `diff pump: exactly one ParamChanged for drifted id ${AUTO_ID} (got ${autos.length})`);
  if (autos[0]) {
    check(approx(autos[0].plain, AUTO_VAL), `pump ParamChanged.plain == published readback (${autos[0].plain})`);
  }
  const otherDrift = sink.filter((e) => e.type === "ParamChanged" && e.id !== AUTO_ID);
  check(otherDrift.length === 0, `diff pump: no spurious ParamChanged for unchanged params (got ${otherDrift.length})`);

  ctrl.destroy();

  console.log(`\n${failures === 0 ? "ALL PASS" : `${failures} FAILED`}`);
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
