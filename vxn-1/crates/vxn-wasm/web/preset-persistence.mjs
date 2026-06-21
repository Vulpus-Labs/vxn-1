// Async-storage <-> sync-controller bridge for user presets (E019 / 0064).
//
// IndexedDB is async; the vxn-app controller + its PresetStore are synchronous,
// drained once per tick. This module is the impedance match the epic calls for:
//
//   - BOOT HYDRATION: read the persisted user corpus out of IndexedDB BEFORE the
//     controller goes live and replay it into the wasm in-memory cache
//     (controller.hydrate*), so list_user_tree / user_load serve synchronously
//     from the cache. The cache is the single source of truth for reads;
//     IndexedDB is a write-behind mirror.
//   - DEFERRED WRITES: after a mutating op the controller mutates the cache
//     synchronously (the corpus snapshot is correct immediately) and journals
//     the persistence op. flush() drains that journal and writes it to IndexedDB
//     OFF the tick — the controller tick never awaits storage I/O. Writes are
//     serialised on a tail promise so rapid edits can't interleave transactions
//     or drop a write, and flush-on-hide is the reload-durability backstop.
//
// This module is the JS owner; the wasm side (vxn-web-controller) owns the cache
// + journal, and preset-storage.mjs is the raw IndexedDB primitive. ONE code
// path: the Node test drives this class against the fake-IDB, the same transport
// the browser runs.

import {
  openPresetDB,
  getAllPresets,
  getAllFolders,
  applyWrites,
} from "./preset-storage.mjs";

export class PresetPersistence {
  // Options:
  //   controller : a WebController (controller.mjs) — already instantiated.
  //   indexedDB  : the IndexedDB factory (seam; defaults to the browser global).
  //   db         : a pre-opened IDBDatabase (the Node test injects the fake's).
  //   openDB / getAllPresets / getAllFolders / applyWrites : preset-storage.mjs
  //                seams (defaults to the real ones).
  constructor({
    controller,
    indexedDB = globalThis.indexedDB,
    db = null,
    openDB = openPresetDB,
    getAllPresets: gap = getAllPresets,
    getAllFolders: gaf = getAllFolders,
    applyWrites: aw = applyWrites,
  } = {}) {
    if (!controller) throw new Error("PresetPersistence needs a controller");
    this.controller = controller;
    this._indexedDB = indexedDB;
    this._db = db;
    this._openDB = openDB;
    this._getAllPresets = gap;
    this._getAllFolders = gaf;
    this._applyWrites = aw;
    // Serialises IndexedDB writes: each flush chains onto the previous so two
    // rapid ops can't race the same store. Also the page-hide drain target.
    this._flushTail = Promise.resolve();
    // False once storage proves unavailable (private mode / blocked / no IDB);
    // the synth still runs, edits just don't persist.
    this._available = true;
  }

  // Open the DB (idempotent). Throws if IndexedDB is unavailable.
  async open() {
    if (this._db) return this._db;
    this._db = await this._openDB(this._indexedDB);
    return this._db;
  }

  // Boot hydration: replay the persisted corpus into the wasm cache, then
  // finish (refresh the corpus snapshot + rebuild the corpus JSON). Resolves
  // true if hydrated, false if storage is unavailable (still calls hydrateDone
  // so the empty corpus is published). Best-effort: a read failure leaves the
  // cache empty rather than blocking boot.
  async hydrate() {
    let db;
    try {
      db = await this.open();
    } catch (e) {
      this._available = false;
      console.warn("vxn: preset storage unavailable; presets won't persist", e && e.message);
      this.controller.hydrateDone();
      return false;
    }
    let folders = [];
    let presets = [];
    try {
      [folders, presets] = await Promise.all([
        this._getAllFolders(db),
        this._getAllPresets(db),
      ]);
    } catch (e) {
      console.warn("vxn: preset hydrate read failed", e);
    }
    for (const name of folders) this.controller.hydrateFolder(name);
    for (const { key, bytes } of presets) this.controller.hydratePreset(key, bytes);
    this.controller.hydrateDone();
    return true;
  }

  // Drain the controller's write journal and flush it to IndexedDB off the tick.
  // The drain (takeJournal) is SYNCHRONOUS so the wasm journal is emptied now;
  // the IndexedDB apply is chained on the tail promise (never awaited by the
  // caller's tick). A storage failure (quota/eviction) is logged, not thrown —
  // the cache stays the source of truth. Returns the tail promise.
  flush() {
    // Always drain, even with no storage, so the journal can't grow unbounded.
    const ops = this.controller.takeJournal();
    if (!this._available || !this._db || ops.length === 0) return this._flushTail;
    this._flushTail = this._flushTail
      .then(() => this._applyWrites(this._db, ops))
      .catch((e) => console.warn("vxn: preset flush failed (quota/eviction?)", e));
    return this._flushTail;
  }

  // Await any in-flight flush (the page-hide path can await this).
  drain() {
    return this._flushTail;
  }

  // Flush on tab-hide / page-unload so a reload immediately after a save can't
  // lose it. visibilitychange→hidden is the reliable foreground→background
  // signal (fires early enough for the async write to land); pagehide is the
  // bfcache-safe unload. Both just call flush() (already-flushed is a no-op).
  attachFlushOnHide(win = globalThis, doc = globalThis.document) {
    if (win && typeof win.addEventListener === "function") {
      this._onPageHide = () => this.flush();
      win.addEventListener("pagehide", this._onPageHide);
    }
    if (doc && typeof doc.addEventListener === "function") {
      this._onVisibility = () => {
        if (doc.visibilityState === "hidden") this.flush();
      };
      doc.addEventListener("visibilitychange", this._onVisibility);
    }
    this._hideWin = win;
    this._hideDoc = doc;
  }

  detachFlushOnHide() {
    if (this._hideWin && this._onPageHide) {
      this._hideWin.removeEventListener("pagehide", this._onPageHide);
    }
    if (this._hideDoc && this._onVisibility) {
      this._hideDoc.removeEventListener("visibilitychange", this._onVisibility);
    }
    this._onPageHide = this._onVisibility = null;
  }
}
