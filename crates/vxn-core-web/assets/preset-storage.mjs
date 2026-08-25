// Browser-storage primitive for user presets.
//
// The web ports replace the desktop std::fs user-preset side with IndexedDB
// (vxn-1 ADR 0009 addendum: chosen over OPFS — the corpus is small key→value
// blobs, IndexedDB is universal, and a flat store fits better than OPFS's file
// tree).
//
// This module is JUST the storage layer: open the DB and read/write the three
// object stores. It is intentionally dumb — no corpus logic, no controller
// coupling. The wasm-side UserState (each port's `user_store.rs`) owns the cache
// + the write journal; preset-persistence.mjs wires the bridge: hydrate the
// cache from getAll() at boot, and flush the journal here via applyWrites().
//
//   store "presets": key = synthetic path ("folder/Name.toml" | "Name.toml"),
//                    value = Uint8Array (the port's preset-record bytes).
//   store "folders": key = folder name (so empty folders persist).
//   store "state":   key = a fixed slot string, value = Uint8Array (the full
//                    patch-state blob — the host-state analogue, autosave).
//
// Values are stored as plain Uint8Array; structured-clone handles them.
//
// SHARED (0284): one copy for every browser port. The **DB identity is not
// shared** — each synth passes its own `{ name, version }` so the corpora never
// collide and no shipped store gets silently renumbered. See `openPresetDB`.

export const STORE_PRESETS = "presets";
export const STORE_FOLDERS = "folders";
export const STORE_STATE = "state";
// The single autosave slot key in STORE_STATE (the "last session" patch).
export const STATE_KEY = "session";

// Open (creating/upgrading) the preset DB. Resolves to the IDBDatabase.
//
// `db` is the caller's DB identity — `{ name, version }`. It is REQUIRED and
// deliberately un-defaulted: the name partitions one synth's corpus from
// another's in the same origin, and the version is that DB's own migration
// history (vxn-1 is at v2 after adding the "state" store; vxn-2 shipped at v1
// with all three stores present). A wrong value here evicts or blocks a user's
// live preset corpus, so there is no safe default to fall back to.
export function openPresetDB(indexedDB = globalThis.indexedDB, db = null) {
  return new Promise((resolve, reject) => {
    if (!indexedDB) {
      reject(new Error("IndexedDB unavailable"));
      return;
    }
    if (!db || !db.name || !db.version) {
      reject(new Error("openPresetDB needs a { name, version } DB identity"));
      return;
    }
    const req = indexedDB.open(db.name, db.version);
    req.onupgradeneeded = () => {
      // `opened`, not `db`: `db` is the identity argument above. Additive and
      // guarded, so a v1 database upgrades in place without losing its presets.
      const opened = req.result;
      if (!opened.objectStoreNames.contains(STORE_PRESETS)) opened.createObjectStore(STORE_PRESETS);
      if (!opened.objectStoreNames.contains(STORE_FOLDERS)) opened.createObjectStore(STORE_FOLDERS);
      if (!opened.objectStoreNames.contains(STORE_STATE)) opened.createObjectStore(STORE_STATE);
    };
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}

// Promise wrapper over a single transaction.
function tx(db, store, mode, fn) {
  return new Promise((resolve, reject) => {
    const t = db.transaction(store, mode);
    const s = t.objectStore(store);
    let result;
    try {
      result = fn(s);
    } catch (e) {
      reject(e);
      return;
    }
    t.oncomplete = () => resolve(result);
    t.onerror = () => reject(t.error);
    t.onabort = () => reject(t.error);
  });
}

// All presets as [{ key, bytes }] (bytes = Uint8Array). Used by preset-
// persistence's boot hydration to seed the wasm cache.
export async function getAllPresets(db) {
  return tx(db, STORE_PRESETS, "readonly", (s) => {
    const out = [];
    s.openCursor().onsuccess = (e) => {
      const cur = e.target.result;
      if (!cur) return;
      out.push({ key: cur.key, bytes: new Uint8Array(cur.value) });
      cur.continue();
    };
    return out;
  });
}

// All folder names (including empty ones).
export async function getAllFolders(db) {
  return tx(db, STORE_FOLDERS, "readonly", (s) => {
    const out = [];
    s.openCursor().onsuccess = (e) => {
      const cur = e.target.result;
      if (!cur) return;
      out.push(cur.key);
      cur.continue();
    };
    return out;
  });
}

export function putPreset(db, key, bytes) {
  return tx(db, STORE_PRESETS, "readwrite", (s) => s.put(bytes, key));
}
export function deletePreset(db, key) {
  return tx(db, STORE_PRESETS, "readwrite", (s) => s.delete(key));
}
export function putFolder(db, name) {
  return tx(db, STORE_FOLDERS, "readwrite", (s) => s.put(1, name));
}
export function deleteFolder(db, name) {
  return tx(db, STORE_FOLDERS, "readwrite", (s) => s.delete(name));
}

// Full patch-state autosave slot. One key→blob entry, the
// host-state analogue. getState resolves the stored Uint8Array or null. The
// `get`'s onsuccess fires before the transaction's oncomplete (which resolves
// tx), so the captured result is ready by then — same shape as getAllPresets'
// cursor accumulation.
export function getState(db, key = STATE_KEY) {
  return tx(db, STORE_STATE, "readonly", (s) => {
    const out = { value: null };
    s.get(key).onsuccess = (e) => {
      const v = e.target.result;
      out.value = v ? new Uint8Array(v) : null;
    };
    return out;
  }).then((out) => out.value);
}
export function putState(db, bytes, key = STATE_KEY) {
  return tx(db, STORE_STATE, "readwrite", (s) => s.put(bytes, key));
}

// Apply a batch of journal ops (the wasm UserState's UserWrite variants, decoded
// JS-side by controller.takeJournal) to the DB. Each op: {kind:'put'|'delete'|
// 'delete_folder', key?, bytes?, name?}. Ops run sequentially; a failure
// rejects so the caller can surface a storage error (quota/eviction).
export async function applyWrites(db, ops) {
  for (const op of ops) {
    switch (op.kind) {
      case "put":
        await putPreset(db, op.key, op.bytes);
        break;
      case "delete":
        await deletePreset(db, op.key);
        break;
      case "put_folder":
        await putFolder(db, op.name);
        break;
      case "delete_folder":
        await deleteFolder(db, op.name);
        break;
      default:
        throw new Error(`preset-storage: unknown write op ${op.kind}`);
    }
  }
}
