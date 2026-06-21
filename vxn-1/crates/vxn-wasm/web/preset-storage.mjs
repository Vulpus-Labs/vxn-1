// Browser-storage primitive for user presets (E019 / 0063).
//
// The web port replaces the desktop std::fs user-preset side with IndexedDB
// (ADR 0009 addendum: chosen over OPFS — the corpus is small key→value blobs,
// IndexedDB is universal, and a flat store fits better than OPFS's file tree).
//
// This module is JUST the storage layer: open the DB and read/write the two
// object stores. It is intentionally dumb — no corpus logic, no controller
// coupling. The wasm-side UserState (user_store.rs) owns the cache + the write
// journal; ticket 0064 wires the bridge: hydrate the cache from getAll() at
// boot, and flush the journal here via applyWrites().
//
//   store "presets": key = synthetic path ("folder/Name.toml" | "Name.toml"),
//                    value = Uint8Array (vxn_app::preset_record bytes).
//   store "folders": key = folder name (so empty folders persist).
//   store "state":   key = a fixed slot string, value = Uint8Array (the full
//                    patch-state blob — the host-state analogue, E019 / 0065).
//
// Values are stored as plain Uint8Array; structured-clone handles them.

export const DB_NAME = "vxn1-presets";
// v2 (0065) adds the "state" store for full-patch autosave. onupgradeneeded is
// additive (guarded createObjectStore), so a v1 db upgrades in place.
export const DB_VERSION = 2;
export const STORE_PRESETS = "presets";
export const STORE_FOLDERS = "folders";
export const STORE_STATE = "state";
// The single autosave slot key in STORE_STATE (the "last session" patch).
export const STATE_KEY = "session";

// Open (creating/upgrading) the preset DB. Resolves to the IDBDatabase.
export function openPresetDB(indexedDB = globalThis.indexedDB) {
  return new Promise((resolve, reject) => {
    if (!indexedDB) {
      reject(new Error("IndexedDB unavailable"));
      return;
    }
    const req = indexedDB.open(DB_NAME, DB_VERSION);
    req.onupgradeneeded = () => {
      const db = req.result;
      if (!db.objectStoreNames.contains(STORE_PRESETS)) db.createObjectStore(STORE_PRESETS);
      if (!db.objectStoreNames.contains(STORE_FOLDERS)) db.createObjectStore(STORE_FOLDERS);
      if (!db.objectStoreNames.contains(STORE_STATE)) db.createObjectStore(STORE_STATE);
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

// All presets as [{ key, bytes }] (bytes = Uint8Array). Used by 0064 boot
// hydration to seed the wasm cache.
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

// Full patch-state autosave slot (E019 / 0065). One key→blob entry, the
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
// JS-side by 0064) to the DB. Each op: {kind:'put'|'delete'|'put_folder'|
// 'delete_folder', key?, bytes?, name?}. Ops run sequentially; a failure
// rejects so 0064 can surface a storage error (quota/eviction).
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
