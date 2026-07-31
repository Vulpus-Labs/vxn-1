// IndexedDB user-preset storage primitive (0159). Run:
//   node --test crates/vxn2-wasm/web/preset-storage.test.mjs
//
// Node has no IndexedDB, so this drives the module against a MINIMAL in-memory
// fake that mimics the bits preset-storage.mjs uses (open + upgrade, a
// readwrite/readonly transaction with put/delete/get/openCursor, and the
// oncomplete/onerror lifecycle). The real DB is browser-verified; this proves
// the wrapper's transaction/cursor wiring + applyWrites batching headlessly.

import { test } from "node:test";
import assert from "node:assert/strict";
import {
  openPresetDB,
  getAllPresets,
  getAllFolders,
  putPreset,
  deletePreset,
  getState,
  putState,
  applyWrites,
  STORE_PRESETS,
  STORE_FOLDERS,
  STORE_STATE,
} from "./preset-storage.mjs";

// ---- minimal in-memory IndexedDB fake --------------------------------------
export function fakeIndexedDB() {
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
        const req = { onsuccess: null, onerror: null, result: undefined };
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

test("put → getAll round-trips bytes by key; delete removes", async () => {
  const db = await openPresetDB(fakeIndexedDB());
  await putPreset(db, "Bass.toml", new Uint8Array([1, 2, 3]));
  await putPreset(db, "Pads/Warm.toml", new Uint8Array([9]));
  let all = await getAllPresets(db);
  assert.equal(all.length, 2);
  const bass = all.find((p) => p.key === "Bass.toml");
  assert.ok(bass && bass.bytes instanceof Uint8Array);
  assert.equal(bass.bytes[0], 1);

  await deletePreset(db, "Bass.toml");
  all = await getAllPresets(db);
  assert.deepEqual(all.map((p) => p.key), ["Pads/Warm.toml"]);
});

test("applyWrites batches the journal-op shapes takeJournal hands us", async () => {
  const db = await openPresetDB(fakeIndexedDB());
  await applyWrites(db, [
    { kind: "put", key: "Lead.toml", bytes: new Uint8Array([7]) },
    { kind: "put_folder", name: "Leads" },
    { kind: "delete", key: "Lead.toml" },
    { kind: "put", key: "Keep.toml", bytes: new Uint8Array([8]) },
  ]);
  const all = await getAllPresets(db);
  const folders = await getAllFolders(db);
  assert.deepEqual(all.map((p) => p.key), ["Keep.toml"]);
  assert.deepEqual(folders, ["Leads"]);
});

test("applyWrites rejects an unknown op kind", async () => {
  const db = await openPresetDB(fakeIndexedDB());
  await assert.rejects(() => applyWrites(db, [{ kind: "bogus" }]));
});

test("state slot: put → get round-trips the autosave blob; missing → null", async () => {
  const db = await openPresetDB(fakeIndexedDB());
  assert.equal(await getState(db), null);
  await putState(db, new Uint8Array([4, 5, 6]));
  const blob = await getState(db);
  assert.ok(blob instanceof Uint8Array && blob.length === 3 && blob[1] === 5);
});
