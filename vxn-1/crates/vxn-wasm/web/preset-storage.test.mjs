// Headless test for the IndexedDB user-preset storage primitive (E019 / 0063).
//
//   node web/preset-storage.test.mjs
//
// Node has no IndexedDB, so this drives the module against a MINIMAL in-memory
// fake that mimics the bits preset-storage.mjs uses (open + upgrade, a
// readwrite/readonly transaction with put/delete/openCursor, and the
// oncomplete/onerror lifecycle). The real DB is browser-verified; this proves
// the wrapper's transaction/cursor wiring + applyWrites batching headlessly.

import {
  openPresetDB,
  getAllPresets,
  getAllFolders,
  putPreset,
  deletePreset,
  applyWrites,
  STORE_PRESETS,
  STORE_FOLDERS,
} from "./preset-storage.mjs";

// ---- minimal in-memory IndexedDB fake --------------------------------------
function fakeIndexedDB() {
  const stores = { [STORE_PRESETS]: new Map(), [STORE_FOLDERS]: new Map() };
  const fireNextTick = (obj, prop) => queueMicrotask(() => obj[prop] && obj[prop]({ target: obj }));

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
            req.onsuccess &&
              req.onsuccess({ target: { result: { key, value, continue: step } } });
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
    transaction: (name, _mode) => {
      const t = { oncomplete: null, onerror: null, onabort: null, objectStore };
      // Resolve the transaction after any queued op microtasks have run.
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

let failures = 0;
const check = (cond, msg) => {
  console.log(`  ${cond ? "PASS" : "FAIL"}  ${msg}`);
  if (!cond) failures++;
};

async function main() {
  const db = await openPresetDB(fakeIndexedDB());

  // put → getAll round-trips bytes by key.
  await putPreset(db, "Bass.toml", new Uint8Array([1, 2, 3]));
  await putPreset(db, "Pads/Warm.toml", new Uint8Array([9]));
  let all = await getAllPresets(db);
  check(all.length === 2, `getAllPresets returns 2 (${all.length})`);
  const bass = all.find((p) => p.key === "Bass.toml");
  check(!!bass && bass.bytes instanceof Uint8Array, "value is a Uint8Array");
  check(bass && bass.bytes.length === 3 && bass.bytes[0] === 1, "bytes round-trip");

  // delete removes one.
  await deletePreset(db, "Bass.toml");
  all = await getAllPresets(db);
  check(all.length === 1 && all[0].key === "Pads/Warm.toml", "delete removed the key");

  // applyWrites batches the journal-op shapes 0064 will hand us.
  await applyWrites(db, [
    { kind: "put", key: "Lead.toml", bytes: new Uint8Array([7]) },
    { kind: "put_folder", name: "Leads" },
    { kind: "delete", key: "Pads/Warm.toml" },
  ]);
  all = await getAllPresets(db);
  const folders = await getAllFolders(db);
  check(all.length === 1 && all[0].key === "Lead.toml", "applyWrites put+delete applied");
  check(folders.length === 1 && folders[0] === "Leads", "applyWrites put_folder applied");

  // unknown op rejects.
  let threw = false;
  try {
    await applyWrites(db, [{ kind: "bogus" }]);
  } catch {
    threw = true;
  }
  check(threw, "applyWrites rejects an unknown op kind");

  console.log(failures === 0 ? "\nALL PASS" : `\n${failures} FAILURE(S)`);
  process.exit(failures === 0 ? 0 : 1);
}

main();
