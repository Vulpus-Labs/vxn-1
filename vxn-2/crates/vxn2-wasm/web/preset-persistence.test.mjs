// Async-storage <-> sync-controller bridge for user presets (0159). Run after
// `cargo xtask web [--debug]`:
//   node --test crates/vxn2-wasm/web/preset-persistence.test.mjs
//
// Drives the REAL vxn2-web-controller wasm + the real PresetPersistence against
// a minimal in-memory IndexedDB fake. Proves the user-preset acceptance criteria
// headlessly:
//   - user presets persist across a "reload": save → flush → a FRESH controller
//     hydrated from the SAME db lists + loads them;
//   - the corpus snapshot is correct SYNCHRONOUSLY after a mutating op (no wait
//     on storage) — corpusJson() reflects the save the same tick;
//   - no write lost under rapid successive saves or a reload right after a save;
//   - storage-unavailable degrades gracefully (synth still runs).
// Skips (not fails) when the web bundle isn't built.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { WebController } from "./controller.mjs";
import { DB_ID } from "./faceplate-bridge.mjs";
import { PresetPersistence } from "../../../../crates/vxn-core-web/assets/preset-persistence.mjs";
import { STORE_PRESETS, STORE_FOLDERS, STORE_STATE } from "../../../../crates/vxn-core-web/assets/preset-storage.mjs";

const WASM = fileURLToPath(new URL("../../../../target/web-dist/vxn2_web_controller.wasm", import.meta.url));
const HAVE = existsSync(WASM);
const wasmBytes = HAVE ? readFileSync(WASM) : null;

// ---- minimal in-memory IndexedDB fake (shared across "sessions") -----------
function fakeIndexedDB() {
  const stores = {
    [STORE_PRESETS]: new Map(),
    [STORE_FOLDERS]: new Map(),
    [STORE_STATE]: new Map(),
  };
  const makeReq = (run) => {
    const req = { onsuccess: null, onerror: null, result: undefined };
    queueMicrotask(() => {
      try {
        req.result = run();
        req.onsuccess && req.onsuccess({ target: req });
      } catch (e) {
        req.error = e;
        req.onerror && req.onerror({ target: req });
      }
    });
    return req;
  };
  const objectStore = (name) => {
    const map = stores[name];
    return {
      put: (value, key) => makeReq(() => map.set(key, value)),
      delete: (key) => makeReq(() => map.delete(key)),
      get: (key) => {
        const req = { onsuccess: null, result: undefined };
        queueMicrotask(() => {
          req.result = map.has(key) ? map.get(key) : undefined;
          req.onsuccess && req.onsuccess({ target: { result: req.result } });
        });
        return req;
      },
      openCursor: () => {
        const req = { onsuccess: null };
        const entries = [...map.entries()];
        let i = 0;
        const step = () => {
          queueMicrotask(() => {
            if (i >= entries.length) {
              req.onsuccess && req.onsuccess({ target: { result: null } });
              return;
            }
            const [key, value] = entries[i++];
            req.onsuccess && req.onsuccess({ target: { result: { key, value, continue: step } } });
          });
        };
        step();
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

async function newController() {
  const c = new WebController({ wasmBytes });
  await c.instantiate();
  return c;
}

// Find a user preset's synthetic path in the corpus JSON by display name.
function findUserPath(corpus, name) {
  for (const folder of corpus.user || []) {
    const hit = (folder.presets || []).find((p) => p.name === name);
    if (hit) return hit.path;
  }
  return null;
}

test("user presets persist across a reload; corpus is synchronous", { skip: !HAVE }, async () => {
  const idb = fakeIndexedDB(); // one db, two controller "sessions"

  // session 1: save presets + a folder, flush to storage.
  const c1 = await newController();
  const p1 = new PresetPersistence({ controller: c1, dbId: DB_ID, indexedDB: idb });
  assert.equal(await p1.hydrate(), true, "empty-corpus hydrate resolves true");

  c1.savePreset("Mini Bass", null);
  c1.newFolder("Leads");
  c1.savePreset("Hero", "Leads");
  c1.savePreset("Rapid", null);
  c1.savePreset("Rapid", null); // rapid duplicate → last wins, no loss
  c1.tick();

  // corpus correct SYNCHRONOUSLY after the tick.
  const corpus1 = c1.corpusJson();
  assert.ok(findUserPath(corpus1, "Mini Bass"), "corpus lists 'Mini Bass' at root immediately");
  const leads1 = (corpus1.user || []).find((f) => f.name === "Leads");
  assert.ok(leads1 && leads1.presets.some((p) => p.name === "Hero"), "corpus lists 'Hero' under 'Leads'");
  const root1 = (corpus1.user || []).find((f) => f.name === null);
  assert.equal(root1.presets.filter((p) => p.name === "Rapid").length, 1, "duplicate rapid saves collapse to one");

  // flush is off the tick; drain it, then the journal is empty.
  await p1.flush();
  await p1.drain();
  assert.equal(c1.takeJournal().length, 0, "journal drained after flush");

  // session 2: a FRESH controller hydrated from the SAME db.
  const c2 = await newController();
  const p2 = new PresetPersistence({ controller: c2, dbId: DB_ID, indexedDB: idb });
  await p2.hydrate();

  const corpus2 = c2.corpusJson();
  const miniPath = findUserPath(corpus2, "Mini Bass");
  const heroPath = findUserPath(corpus2, "Hero");
  assert.ok(miniPath, "'Mini Bass' survives the reload");
  assert.ok(heroPath && heroPath.includes("Leads"), "'Hero' survives under its folder");
  assert.ok(findUserPath(corpus2, "Rapid"), "'Rapid' survives the reload");

  // the hydrated preset is loadable — loadUser → preset_loaded(user).
  c2.loadUser(miniPath);
  const evs = c2.tick();
  const pl = evs.find((e) => e.kind === "preset_loaded");
  assert.ok(pl && pl.source && pl.source.kind === "user" && pl.source.path === miniPath, "hydrated preset loads");
});

test("delete persists across a reload", { skip: !HAVE }, async () => {
  const idb = fakeIndexedDB();
  const c1 = await newController();
  const p1 = new PresetPersistence({ controller: c1, dbId: DB_ID, indexedDB: idb });
  await p1.hydrate();
  c1.savePreset("Gone", null);
  c1.savePreset("Stay", null);
  c1.tick();
  await p1.flush();
  await p1.drain();
  const gonePath = findUserPath(c1.corpusJson(), "Gone");

  c1.deletePreset(gonePath);
  c1.tick();
  await p1.flush();
  await p1.drain();
  assert.ok(!findUserPath(c1.corpusJson(), "Gone"), "delete reflected in corpus synchronously");

  const c2 = await newController();
  const p2 = new PresetPersistence({ controller: c2, dbId: DB_ID, indexedDB: idb });
  await p2.hydrate();
  assert.ok(!findUserPath(c2.corpusJson(), "Gone"), "delete persists across a reload");
  assert.ok(findUserPath(c2.corpusJson(), "Stay"), "the surviving preset is still there");
});

test("flush-on-hide persists a pending save", { skip: !HAVE }, async () => {
  const idb = fakeIndexedDB();
  const listeners = {};
  const fakeWin = { addEventListener: (ev, cb) => (listeners["win:" + ev] = cb), removeEventListener: () => {} };
  const fakeDoc = {
    visibilityState: "hidden",
    addEventListener: (ev, cb) => (listeners["doc:" + ev] = cb),
    removeEventListener: () => {},
  };
  const c1 = await newController();
  const p1 = new PresetPersistence({ controller: c1, dbId: DB_ID, indexedDB: idb });
  await p1.hydrate();
  p1.attachFlushOnHide(fakeWin, fakeDoc);
  c1.savePreset("HideSaved", null);
  c1.tick();
  listeners["doc:visibilitychange"](); // tab hides → flush
  await p1.drain();

  const c2 = await newController();
  const p2 = new PresetPersistence({ controller: c2, dbId: DB_ID, indexedDB: idb });
  await p2.hydrate();
  assert.ok(findUserPath(c2.corpusJson(), "HideSaved"), "flush-on-hide persisted the pending save");
});

test("storage unavailable degrades gracefully", { skip: !HAVE }, async () => {
  const c = await newController();
  const p = new PresetPersistence({ controller: c, dbId: DB_ID, indexedDB: null }); // no IDB
  assert.equal(await p.hydrate(), false, "hydrate false when storage unavailable");
  assert.doesNotThrow(() => {
    c.savePreset("NoStore", null);
    c.tick();
    p.flush(); // must not throw, just drains
  });
});
