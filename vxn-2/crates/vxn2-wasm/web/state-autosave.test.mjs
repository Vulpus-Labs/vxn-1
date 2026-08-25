// Full patch-state autosave + restore (0159). Run after `cargo xtask web`:
//   node --test crates/vxn2-wasm/web/state-autosave.test.mjs
//
// Drives the REAL vxn2-web-controller wasm + the real StateAutosave against a
// minimal in-memory IndexedDB fake (with the "state" store) and a manual timer
// driver. Proves the autosave acceptance criteria headlessly:
//   - edit params, autosave, then a FRESH controller restored from the SAME db
//     reproduces the EXACT patch (re-snapshot is byte-identical);
//   - a fresh page with no saved state restores false and boots to defaults;
//   - a corrupt / wrong-length blob is ignored gracefully;
//   - autosave never blocks the tick: schedule() debounces, flush() snapshots
//     synchronously + chains the write off the tick.
// Skips (not fails) when the web bundle isn't built.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { WebController } from "./controller.mjs";
import { DB_ID } from "./faceplate-bridge.mjs";
import { StateAutosave } from "../../../../crates/vxn-core-web/assets/state-autosave.mjs";
import { STORE_PRESETS, STORE_FOLDERS, STORE_STATE } from "../../../../crates/vxn-core-web/assets/preset-storage.mjs";

const WASM = fileURLToPath(new URL("../../../../target/web-dist/vxn2_web_controller.wasm", import.meta.url));
const HAVE = existsSync(WASM);
const wasmBytes = HAVE ? readFileSync(WASM) : null;

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
        const req = { onsuccess: null };
        queueMicrotask(() => {
          map.set(key, value);
          req.onsuccess && req.onsuccess({ target: req });
        });
        return req;
      },
      get: (key) => {
        const req = { onsuccess: null };
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
    flushTimers: () => {
      for (const [h, fn] of [...pending.entries()]) {
        pending.delete(h);
        fn();
      }
    },
    pendingCount: () => pending.size,
  };
}

async function newController() {
  const c = new WebController({ wasmBytes });
  await c.instantiate();
  return c;
}

const eq = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);

test("autosave → restore reproduces the exact patch across a reload", { skip: !HAVE }, async () => {
  const idb = fakeIndexedDB();

  // session 1: edit the patch, autosave it.
  const c1 = await newController();
  const timers1 = manualTimers();
  const a1 = new StateAutosave({ controller: c1, dbId: DB_ID, indexedDB: idb, ...timers1 });
  assert.equal(await a1.restore(), false, "fresh db: restore returns false");

  c1.setParamNorm(0, 0.42);
  c1.setParamNorm(10, 0.73);
  c1.tick();
  const blob1 = c1.snapshotState();
  assert.ok(blob1.length > 8, "snapshot produced a blob");

  a1.schedule();
  assert.equal(timers1.pendingCount(), 1, "schedule arms a single debounce timer");
  timers1.flushTimers(); // debounce window elapses → flush()
  await a1.drain();
  assert.equal(timers1.pendingCount(), 0, "timer cleared after flush");

  // session 2: a FRESH controller restored from the SAME db.
  const c2 = await newController();
  const a2 = new StateAutosave({ controller: c2, dbId: DB_ID, indexedDB: idb });
  assert.equal(await a2.restore(), true, "restore returns true (saved state applied)");
  assert.ok(eq(blob1, c2.snapshotState()), "re-snapshot is byte-identical to the saved patch");
});

test("corrupt / short blobs are rejected, model left at defaults", { skip: !HAVE }, async () => {
  const c = await newController();
  const def = c.snapshotState();
  assert.equal(c.restoreState(new Uint8Array(4)), false, "short blob rejected");
  assert.equal(c.restoreState(new Uint8Array(def.length)), false, "right-length garbage rejected (bad magic)");
  assert.ok(eq(c.snapshotState(), def), "model left at defaults after a rejected restore");
});

test("flush-on-hide writes the latest patch", { skip: !HAVE }, async () => {
  const idb = fakeIndexedDB();
  const listeners = {};
  const fakeWin = { addEventListener: (ev, cb) => (listeners["win:" + ev] = cb), removeEventListener: () => {} };
  const fakeDoc = {
    visibilityState: "hidden",
    addEventListener: (ev, cb) => (listeners["doc:" + ev] = cb),
    removeEventListener: () => {},
  };
  const c1 = await newController();
  const t1 = manualTimers();
  const a1 = new StateAutosave({ controller: c1, dbId: DB_ID, indexedDB: idb, ...t1 });
  await a1.restore();
  a1.attachFlushOnHide(fakeWin, fakeDoc);
  c1.setParamNorm(5, 0.123);
  c1.tick();
  a1.schedule(); // a debounce is pending...
  listeners["doc:visibilitychange"](); // ...the tab hides before it fires
  assert.equal(t1.pendingCount(), 0, "flush-on-hide cancels the pending debounce");
  await a1.drain();

  const c2 = await newController();
  const a2 = new StateAutosave({ controller: c2, dbId: DB_ID, indexedDB: idb });
  assert.equal(await a2.restore(), true, "flush-on-hide persisted the latest patch");
  assert.ok(eq(c1.snapshotState(), c2.snapshotState()), "the persisted patch matches the edited one");
});

test("storage unavailable degrades gracefully", { skip: !HAVE }, async () => {
  const c = await newController();
  const a = new StateAutosave({ controller: c, dbId: DB_ID, indexedDB: null }); // no IDB
  assert.equal(await a.restore(), false, "restore false when storage unavailable");
  assert.doesNotThrow(() => {
    c.setParam(0, 0.9);
    c.tick();
    a.schedule(); // no-op without storage
    a.flush(); // must not throw
  });
});
