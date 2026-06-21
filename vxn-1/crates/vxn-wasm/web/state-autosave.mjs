// Full patch-state autosave + restore (E019 / 0065).
//
// On desktop the HOST persists the plugin-state blob; on the web there is no
// host, so the page owns it. This module autosaves the live patch to browser
// storage on change (debounced + flushed on tab-hide) and restores it on the
// next page load — the host-state-blob analogue. It is DISTINCT from user
// presets (0063/0064): one fixed "last session" slot, not a named corpus entry.
//
//   - RESTORE: at boot, before the faceplate's EditorReady re-broadcast, read
//     the saved blob and load it into the model via controller.restoreState().
//     The re-broadcast then seeds the UI + param SAB with the restored values
//     (params + key mode + split point all ride the one canonical blob). Falls
//     back to defaults if there is no blob or it is malformed/wrong-length
//     (restoreState rejects bad blobs, leaving the model at defaults).
//   - AUTOSAVE: schedule() marks the patch dirty and debounces; when the timer
//     fires (or on tab-hide) flush() snapshots the model SYNCHRONOUSLY and
//     writes it OFF the tick, serialised on a tail promise so rapid edits can't
//     interleave transactions. The controller tick never awaits storage I/O.
//
// Same write-behind discipline and seams as preset-persistence.mjs (0064), over
// the SAME IndexedDB (a dedicated "state" store, ADR 0009 addendum): one storage
// mechanism, one more keyed entry. ONE code path — the Node test drives this
// class against the fake-IDB, the same transport the browser runs.

import { openPresetDB, getState, putState } from "./preset-storage.mjs";

export class StateAutosave {
  // Options:
  //   controller : a WebController (controller.mjs) — already instantiated.
  //   indexedDB  : the IndexedDB factory (seam; defaults to the browser global).
  //   db         : a pre-opened IDBDatabase (the Node test injects the fake's).
  //   openDB / getState / putState : preset-storage.mjs seams (defaults real).
  //   debounceMs : quiet period after the last change before a write (default 400).
  //   setTimer / clearTimer : timer seam (defaults to the browser globals; the
  //                Node test injects a manual driver).
  constructor({
    controller,
    indexedDB = globalThis.indexedDB,
    db = null,
    openDB = openPresetDB,
    getState: gs = getState,
    putState: ps = putState,
    debounceMs = 400,
    setTimer = (fn, ms) => setTimeout(fn, ms),
    clearTimer = (h) => clearTimeout(h),
  } = {}) {
    if (!controller) throw new Error("StateAutosave needs a controller");
    this.controller = controller;
    this._indexedDB = indexedDB;
    this._db = db;
    this._openDB = openDB;
    this._getState = gs;
    this._putState = ps;
    this._debounceMs = debounceMs;
    this._setTimer = setTimer;
    this._clearTimer = clearTimer;
    // Serialises writes: each flush chains onto the previous so two rapid writes
    // can't race the store. Also the page-hide drain target.
    this._writeTail = Promise.resolve();
    // Pending debounce timer handle (null when idle).
    this._timer = null;
    // False once storage proves unavailable (private mode / blocked / no IDB);
    // the synth still runs, the patch just isn't autosaved.
    this._available = true;
  }

  // Open the DB (idempotent). Throws if IndexedDB is unavailable.
  async open() {
    if (this._db) return this._db;
    this._db = await this._openDB(this._indexedDB);
    return this._db;
  }

  // Boot restore: load the saved blob into the model. Resolves true if a blob was
  // applied, false if there was none / it was malformed / storage is unavailable
  // (the model stays at defaults). Best-effort: any read failure falls back to
  // defaults rather than blocking boot.
  async restore() {
    let db;
    try {
      db = await this.open();
    } catch (e) {
      this._available = false;
      console.warn("vxn: state storage unavailable; patch won't persist", e && e.message);
      return false;
    }
    let blob = null;
    try {
      blob = await this._getState(db);
    } catch (e) {
      console.warn("vxn: state restore read failed", e);
      return false;
    }
    if (!blob || blob.length === 0) return false;
    // restoreState rejects a malformed / wrong-length blob (returns false),
    // leaving the model at defaults.
    return this.controller.restoreState(blob);
  }

  // Mark the patch dirty and (re)arm the debounce. Coalesces a burst of edits
  // into a single write once edits go quiet for `debounceMs`.
  schedule() {
    if (!this._available || !this._db) return;
    if (this._timer != null) this._clearTimer(this._timer);
    this._timer = this._setTimer(() => {
      this._timer = null;
      this.flush();
    }, this._debounceMs);
  }

  // Snapshot the model NOW and write it off the tick. Cancels any pending
  // debounce (this captures the latest state). The snapshot is synchronous; the
  // IndexedDB put is chained on the tail promise (never awaited by the tick). A
  // storage failure (quota/eviction) is logged, not thrown. Returns the tail.
  flush() {
    if (this._timer != null) {
      this._clearTimer(this._timer);
      this._timer = null;
    }
    if (!this._available || !this._db) return this._writeTail;
    const blob = this.controller.snapshotState();
    this._writeTail = this._writeTail
      .then(() => this._putState(this._db, blob))
      .catch((e) => console.warn("vxn: state autosave failed (quota/eviction?)", e));
    return this._writeTail;
  }

  // Await any in-flight write (the page-hide path can await this).
  drain() {
    return this._writeTail;
  }

  // Flush on tab-hide / page-unload so a reload immediately after an edit keeps
  // it. visibilitychange→hidden is the reliable foreground→background signal;
  // pagehide is the bfcache-safe unload. Both flush() now (snapshotting latest).
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
