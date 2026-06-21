// Headless test for the async-storage <-> sync-controller bridge (E019 / 0064).
//
//   cargo build -p vxn-web-controller --target wasm32-unknown-unknown
//   node web/preset-persistence.test.mjs
//
// Drives the REAL vxn-web-controller wasm + the real PresetPersistence against a
// minimal in-memory IndexedDB fake (the same fake shape preset-storage.test.mjs
// uses). Proves the four 0064 acceptance criteria headlessly:
//
//   AC1  user presets persist across a "reload": save → flush → a FRESH
//        controller hydrated from the SAME db lists + loads them.
//   AC2  the corpus snapshot is correct SYNCHRONOUSLY after a mutating op (no
//        wait on storage) — corpusJson() reflects the save the same tick.
//   AC3  no write lost under rapid successive saves, nor a reload right after a
//        save (the flush ran off the tick; hydrate sees every write).
//   AC4  the tick never blocks on storage — flush() is async + chained; the
//        controller tick path is synchronous (no await in takeJournal).

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { createParamSAB, ParamStore } from "./param-store.mjs";
import { WebController } from "./controller.mjs";
import { PresetPersistence } from "./preset-persistence.mjs";
import { STORE_PRESETS, STORE_FOLDERS } from "./preset-storage.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const WASM = join(
  here,
  "../../../../target/wasm32-unknown-unknown/debug/vxn_web_controller.wasm",
);

// ---- minimal in-memory IndexedDB fake (shared across "sessions") -----------
// Same wiring as preset-storage.test.mjs: open+upgrade, a readonly/readwrite
// transaction with put/delete/openCursor, and the oncomplete lifecycle.
function fakeIndexedDB() {
  const stores = { [STORE_PRESETS]: new Map(), [STORE_FOLDERS]: new Map() };
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

// Find a user preset's synthetic path in the corpus JSON by display name.
function findUserPath(corpus, name) {
  for (const folder of corpus.user || []) {
    const hit = (folder.presets || []).find((p) => p.name === name);
    if (hit) return hit.path;
  }
  return null;
}

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

  const idb = fakeIndexedDB(); // one db, two controller "sessions"

  // ---- session 1: save presets + a folder, flush to storage ----------------
  const c1 = await newController(wasmBytes);
  const p1 = new PresetPersistence({ controller: c1, indexedDB: idb });
  const hydrated1 = await p1.hydrate();
  check(hydrated1 === true, "session 1 hydrate resolves true (empty corpus)");

  // Post a batch of mutating ops, then ONE tick processes them all (FIFO).
  c1.savePreset("Mini Bass", null);
  c1.newFolder("Leads");
  c1.savePreset("Hero", "Leads");
  // AC3 (rapid successive saves of the same name → last wins, no loss).
  c1.savePreset("Rapid", null);
  c1.savePreset("Rapid", null);
  c1.tick();

  // AC2: the corpus is correct SYNCHRONOUSLY after the tick — no storage wait.
  const corpus1 = c1.corpusJson();
  const root1 = (corpus1.user || []).find((f) => f.name === null);
  check(!!findUserPath(corpus1, "Mini Bass"), "AC2 corpus lists 'Mini Bass' at root immediately");
  const leads1 = (corpus1.user || []).find((f) => f.name === "Leads");
  check(!!leads1 && leads1.presets.some((p) => p.name === "Hero"), "AC2 corpus lists 'Hero' under 'Leads'");
  check(
    root1 && root1.presets.filter((p) => p.name === "Rapid").length === 1,
    "AC3 duplicate rapid saves collapse to one entry",
  );

  // AC4: flush is off the tick. Drain it now (the page would do this on rAF /
  // hide). takeJournal() inside flush emptied the wasm journal synchronously.
  await p1.flush();
  await p1.drain();
  check(c1.takeJournal().length === 0, "AC4 write journal drained after flush (no await in tick path)");

  // ---- session 2: a FRESH controller hydrated from the SAME db -------------
  const c2 = await newController(wasmBytes);
  const p2 = new PresetPersistence({ controller: c2, indexedDB: idb });
  await p2.hydrate();

  const corpus2 = c2.corpusJson();
  const miniPath = findUserPath(corpus2, "Mini Bass");
  const heroPath = findUserPath(corpus2, "Hero");
  check(!!miniPath, "AC1 'Mini Bass' survives the reload (listed after hydrate)");
  check(!!heroPath && heroPath.includes("Leads"), "AC1 'Hero' survives under its folder");
  check(!!findUserPath(corpus2, "Rapid"), "AC1 'Rapid' survives the reload");

  // AC1: the hydrated preset is loadable — loadUser → PresetLoaded(user).
  c2.loadUser(miniPath);
  const evs = c2.tick();
  const pl = evs.find((e) => e.type === "PresetLoaded");
  check(
    !!pl && pl.source && pl.source.kind === "user" && pl.source.path === miniPath,
    `AC1 hydrated preset loads (PresetLoaded user#${pl && pl.source && pl.source.path})`,
  );

  // ---- delete persists across a reload too ---------------------------------
  c2.deletePreset(miniPath);
  c2.tick();
  await p2.flush();
  await p2.drain();
  check(!findUserPath(c2.corpusJson(), "Mini Bass"), "delete reflected in corpus synchronously");

  const c3 = await newController(wasmBytes);
  const p3 = new PresetPersistence({ controller: c3, indexedDB: idb });
  await p3.hydrate();
  check(!findUserPath(c3.corpusJson(), "Mini Bass"), "AC1 delete persists across a reload");
  check(!!findUserPath(c3.corpusJson(), "Hero"), "AC1 the surviving preset is still there");

  // ---- flush-on-hide wiring fires flush ------------------------------------
  {
    const listeners = {};
    const fakeWin = { addEventListener: (ev, cb) => (listeners["win:" + ev] = cb), removeEventListener: () => {} };
    const fakeDoc = {
      visibilityState: "hidden",
      addEventListener: (ev, cb) => (listeners["doc:" + ev] = cb),
      removeEventListener: () => {},
    };
    const c4 = await newController(wasmBytes);
    const p4 = new PresetPersistence({ controller: c4, indexedDB: idb });
    await p4.hydrate();
    p4.attachFlushOnHide(fakeWin, fakeDoc);
    c4.savePreset("HideSaved", null);
    c4.tick();
    // The op journalled; simulate the tab going hidden → flush.
    listeners["doc:visibilitychange"]();
    await p4.drain();
    const c5 = await newController(wasmBytes);
    const p5 = new PresetPersistence({ controller: c5, indexedDB: idb });
    await p5.hydrate();
    check(!!findUserPath(c5.corpusJson(), "HideSaved"), "flush-on-hide persisted the pending save");
  }

  // ---- storage unavailable degrades gracefully -----------------------------
  {
    const cX = await newController(wasmBytes);
    const pX = new PresetPersistence({ controller: cX, indexedDB: null }); // no IDB
    const ok = await pX.hydrate();
    check(ok === false, "hydrate returns false when storage is unavailable");
    let threw = false;
    try {
      cX.savePreset("NoStore", null);
      cX.tick();
      pX.flush(); // must not throw, just drains
    } catch {
      threw = true;
    }
    check(!threw, "save + flush without storage does not throw (synth still runs)");
  }

  console.log(failures === 0 ? "\nALL PASS" : `\n${failures} FAILURE(S)`);
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
