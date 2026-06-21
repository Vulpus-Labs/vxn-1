// Headless test for full patch-state autosave + restore (E019 / 0065).
//
//   cargo build -p vxn-web-controller --target wasm32-unknown-unknown
//   node web/state-autosave.test.mjs
//
// Drives the REAL vxn-web-controller wasm + the real StateAutosave against a
// minimal in-memory IndexedDB fake (the same fake shape preset-persistence.test
// uses, with the 0065 "state" store added). Proves the four 0065 acceptance
// criteria headlessly:
//
//   AC1  edit params + key mode + split, autosave, then a FRESH controller
//        restored from the SAME db reproduces the EXACT patch (re-snapshot is
//        byte-identical), and the restored values reach the param SAB through
//        the EditorReady re-broadcast.
//   AC2  a fresh page with no saved state restores false and boots to defaults.
//   AC3  a corrupt / wrong-length blob is ignored gracefully (restore false,
//        model at defaults, no throw).
//   AC4  autosave never blocks the tick: schedule() debounces, flush() snapshots
//        synchronously + chains the write; the tick path takes no await.

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { createParamSAB, ParamStore } from "./param-store.mjs";
import { WebController, KEY_MODE_SPLIT } from "./controller.mjs";
import { StateAutosave } from "./state-autosave.mjs";
import { STORE_PRESETS, STORE_FOLDERS, STORE_STATE } from "./preset-storage.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const WASM = join(
  here,
  "../../../../target/wasm32-unknown-unknown/debug/vxn_web_controller.wasm",
);

// ---- minimal in-memory IndexedDB fake (shared across "sessions") -----------
// Same wiring as preset-persistence.test.mjs, plus a `get` op + the STORE_STATE
// store the 0065 autosave slot uses.
function fakeIndexedDB() {
  const stores = {
    [STORE_PRESETS]: new Map(),
    [STORE_FOLDERS]: new Map(),
    [STORE_STATE]: new Map(),
  };
  const objectStore = (name) => {
    const map = stores[name];
    return {
      put: (value, key) => {
        const req = { onsuccess: null, onerror: null };
        queueMicrotask(() => {
          map.set(key, value);
          req.onsuccess && req.onsuccess({ target: req });
        });
        return req;
      },
      get: (key) => {
        const req = { onsuccess: null, onerror: null };
        queueMicrotask(() => {
          req.result = map.has(key) ? map.get(key) : undefined;
          req.onsuccess && req.onsuccess({ target: { result: req.result } });
        });
        return req;
      },
    };
  };
  const db = {
    objectStoreNames: { contains: (n) => n in stores },
    createObjectStore: () => {},
    transaction: () => {
      const t = { oncomplete: null, onerror: null, onabort: null, objectStore };
      // oncomplete after the request microtasks (get/put fire on 1 microtask).
      queueMicrotask(() => queueMicrotask(() => t.oncomplete && t.oncomplete()));
      return t;
    },
  };
  return {
    open: () => {
      const req = { onupgradeneeded: null, onsuccess: null, onerror: null, result: db };
      queueMicrotask(() => {
        req.onupgradeneeded && req.onupgradeneeded({ target: req });
        req.onsuccess && req.onsuccess({ target: req });
      });
      return req;
    },
  };
}

// A manual timer driver so the debounce is deterministic (no real setTimeout).
function manualTimers() {
  let next = 1;
  const pending = new Map();
  return {
    setTimer: (fn) => {
      const h = next++;
      pending.set(h, fn);
      return h;
    },
    clearTimer: (h) => pending.delete(h),
    // Fire every armed timer (the debounce window elapsing).
    flushTimers: () => {
      for (const [h, fn] of [...pending.entries()]) {
        pending.delete(h);
        fn();
      }
    },
    pendingCount: () => pending.size,
  };
}

let failures = 0;
const check = (cond, msg) => {
  console.log(`  ${cond ? "PASS" : "FAIL"}  ${msg}`);
  if (!cond) failures++;
};

async function newController(wasmBytes) {
  const store = new ParamStore(createParamSAB());
  const c = new WebController({ wasmBytes, store });
  await c.instantiate();
  return c;
}

const eq = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);

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

  const idb = fakeIndexedDB(); // one db, several controller "sessions"

  // ---- session 1: edit the patch, autosave it ------------------------------
  const store1 = new ParamStore(createParamSAB());
  const c1 = new WebController({ wasmBytes, store: store1 });
  await c1.instantiate();
  const timers1 = manualTimers();
  const a1 = new StateAutosave({ controller: c1, indexedDB: idb, ...timers1 });
  const restored1 = await a1.restore();
  check(restored1 === false, "AC2 fresh db: restore returns false (no saved state)");

  // Edit a couple of params, key mode, and split point. (Normalised sets so the
  // values survive any per-descriptor quantisation — the round-trip is then
  // exact regardless of param kind.)
  c1.setParamNorm(0, 0.42);
  c1.setParamNorm(10, 0.73);
  c1.setKeyMode(KEY_MODE_SPLIT);
  c1.setSplitPoint(48);
  c1.tick(); // model now holds the edits
  const ref0 = store1.read(0);
  const ref10 = store1.read(10);

  const blob1 = c1.snapshotState();
  check(blob1.length > 8, `snapshot produced a blob (${blob1.length} bytes)`);

  // AC4: schedule debounces — nothing written until the timer fires; the tick
  // path itself never awaited storage.
  a1.schedule();
  check(timers1.pendingCount() === 1, "AC4 schedule arms a single debounce timer");
  timers1.flushTimers(); // debounce window elapses -> flush()
  await a1.drain();
  check(timers1.pendingCount() === 0, "AC4 timer cleared after flush");

  // ---- session 2: a FRESH controller restored from the SAME db -------------
  const store2 = new ParamStore(createParamSAB());
  const c2 = new WebController({ wasmBytes, store: store2 });
  await c2.instantiate();
  const a2 = new StateAutosave({ controller: c2, indexedDB: idb });
  const restored2 = await a2.restore();
  check(restored2 === true, "AC1 restore returns true (saved state applied)");

  // Re-snapshot must be byte-identical to session 1 (exact patch round-trip:
  // params + key mode + split point).
  const blob2 = c2.snapshotState();
  check(eq(blob1, blob2), "AC1 re-snapshot is byte-identical to the saved patch");

  // The restored values reach the param SAB via the EditorReady re-broadcast.
  c2.editorReady();
  c2.tick();
  check(Math.abs(store2.read(0) - ref0) < 1e-6, "AC1 restored param 0 seeded into the SAB");
  check(Math.abs(store2.read(10) - ref10) < 1e-6, "AC1 restored param 10 seeded into the SAB");

  // ---- AC3: a corrupt blob is ignored gracefully ---------------------------
  {
    const cBad = await newController(wasmBytes);
    const def = cBad.snapshotState(); // cold defaults
    check(cBad.restoreState(new Uint8Array(4)) === false, "AC3 short blob rejected");
    check(cBad.restoreState(new Uint8Array(blob1.length)) === false, "AC3 right-length garbage rejected (bad magic)");
    check(eq(cBad.snapshotState(), def), "AC3 model left at defaults after a rejected restore");
  }

  // ---- flush-on-hide writes the latest patch -------------------------------
  {
    const listeners = {};
    const fakeWin = { addEventListener: (ev, cb) => (listeners["win:" + ev] = cb), removeEventListener: () => {} };
    const fakeDoc = {
      visibilityState: "hidden",
      addEventListener: (ev, cb) => (listeners["doc:" + ev] = cb),
      removeEventListener: () => {},
    };
    const storeH = new ParamStore(createParamSAB());
    const cH = new WebController({ wasmBytes, store: storeH });
    await cH.instantiate();
    const tH = manualTimers();
    const aH = new StateAutosave({ controller: cH, indexedDB: idb, ...tH });
    await aH.restore();
    aH.attachFlushOnHide(fakeWin, fakeDoc);
    cH.setParamNorm(5, 0.123);
    cH.tick();
    const refH = storeH.read(5);
    aH.schedule(); // a debounce is pending...
    listeners["doc:visibilitychange"](); // ...the tab hides before it fires
    check(tH.pendingCount() === 0, "flush-on-hide cancels the pending debounce");
    await aH.drain();

    const storeR = new ParamStore(createParamSAB());
    const cR = new WebController({ wasmBytes, store: storeR });
    await cR.instantiate();
    const aR = new StateAutosave({ controller: cR, indexedDB: idb });
    check((await aR.restore()) === true, "flush-on-hide persisted the latest patch");
    cR.editorReady();
    cR.tick();
    check(Math.abs(storeR.read(5) - refH) < 1e-6, "flush-on-hide preserved the latest edit");
  }

  // ---- storage unavailable degrades gracefully -----------------------------
  {
    const cX = await newController(wasmBytes);
    const aX = new StateAutosave({ controller: cX, indexedDB: null }); // no IDB
    const ok = await aX.restore();
    check(ok === false, "restore returns false when storage is unavailable");
    let threw = false;
    try {
      cX.setParam(0, 0.9);
      cX.tick();
      aX.schedule(); // no-op without storage
      aX.flush(); // must not throw
    } catch {
      threw = true;
    }
    check(!threw, "schedule + flush without storage does not throw (synth still runs)");
  }

  console.log(failures === 0 ? "\nALL PASS" : `\n${failures} FAILURE(S)`);
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
