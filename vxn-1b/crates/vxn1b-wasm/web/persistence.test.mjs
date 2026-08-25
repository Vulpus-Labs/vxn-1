// Browser persistence wiring (ticket 0293).
//
//   node --test vxn-1b/crates/vxn1b-wasm/web/persistence.test.mjs
//
// Drives the real controller wasm and the real shared modules over an in-memory
// IndexedDB fake, so a "session" is a fresh controller against the same store.
// FAILS (never skips) if the wasm is not built.

import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

import { FaceplateBridge, boot, DB_ID } from "./faceplate-bridge.mjs";
import { WebController } from "./controller.mjs";
import { ParamStore, createParamSAB, TOTAL_PARAMS, patchClapId } from "./param-store.mjs";
import { LAYER_L1 } from "./event-codec.mjs";
import { PresetPersistence } from "../../../../crates/vxn-core-web/assets/preset-persistence.mjs";
import { StateAutosave } from "../../../../crates/vxn-core-web/assets/state-autosave.mjs";
import * as patchIo from "../../../../crates/vxn-core-web/assets/patch-io.mjs";
import {
  STORE_PRESETS,
  STORE_FOLDERS,
  STORE_STATE,
} from "../../../../crates/vxn-core-web/assets/preset-storage.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const wasmBytes = await readFile(
  path.resolve(here, "../../../../target/wasm32-unknown-unknown/release/vxn1b_web_controller.wasm"),
).catch(() => {
  throw new Error("controller wasm not built — build it, do not skip this test");
});
const engineWasm = await readFile(
  path.resolve(here, "../../../../target/wasm32-unknown-unknown/release/vxn1b_wasm.wasm"),
).catch(() => null);

const CUTOFF = patchClapId(LAYER_L1, 19);

// ---- in-memory IndexedDB fake, shared across "sessions" --------------------
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
    stores,
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

const newController = async () =>
  new WebController({ wasmBytes, store: new ParamStore(createParamSAB()) }).instantiate();

function findUserPath(corpus, name) {
  for (const folder of corpus.user || []) {
    const hit = (folder.presets || []).find((p) => p.name === name);
    if (hit) return hit.path;
  }
  return null;
}

test("a user preset survives a reload, with its folder and its sound", async () => {
  const idb = fakeIndexedDB();

  // Session 1: save into a folder, flush.
  const a = await newController();
  a.tick();
  a.setParam(CUTOFF, 812);
  a.tick();
  const pa = new PresetPersistence({ controller: a, indexedDB: idb, dbId: DB_ID });
  await pa.hydrate();
  a.savePreset("Reloaded", "Leads");
  a.tick();
  await pa.flush();
  await pa.drain();
  a.destroy();

  // Session 2: a fresh controller over the same store.
  const b = await newController();
  const pb = new PresetPersistence({ controller: b, indexedDB: idb, dbId: DB_ID });
  await pb.hydrate();
  const corpus = b.corpusJson();
  const p = findUserPath(corpus, "Reloaded");
  assert.ok(p, `preset missing after reload: ${JSON.stringify(corpus.user)}`);
  assert.ok(p.startsWith("Leads/"), `folder lost: ${p}`);

  b.tick();
  b.loadUser(p);
  const loaded = b.tick().find((e) => e.kind === "param_changed" && e.id === CUTOFF);
  assert.ok(loaded, "loading the reloaded preset produced no re-broadcast");
  assert.ok(Math.abs(loaded.plain - 812) < 1, `sound not preserved: ${loaded.plain}`);
  b.destroy();
});

test("rename, move and delete survive a reload", async () => {
  const idb = fakeIndexedDB();
  const a = await newController();
  a.tick();
  const pa = new PresetPersistence({ controller: a, indexedDB: idb, dbId: DB_ID });
  await pa.hydrate();
  a.savePreset("First", null);
  a.tick();
  a.renamePreset("First.toml", "Second");
  a.tick();
  a.movePreset("Second.toml", "Pads");
  a.tick();
  await pa.flush();
  await pa.drain();
  a.destroy();

  const b = await newController();
  const pb = new PresetPersistence({ controller: b, indexedDB: idb, dbId: DB_ID });
  await pb.hydrate();
  let corpus = b.corpusJson();
  assert.equal(findUserPath(corpus, "First"), null, "the old name came back");
  assert.equal(findUserPath(corpus, "Second"), "Pads/Second.toml");

  // …and a delete is not resurrected either.
  b.tick();
  b.deletePreset("Pads/Second.toml");
  b.tick();
  await pb.flush();
  await pb.drain();
  b.destroy();

  const c = await newController();
  const pc = new PresetPersistence({ controller: c, indexedDB: idb, dbId: DB_ID });
  await pc.hydrate();
  assert.equal(findUserPath(c.corpusJson(), "Second"), null, "deleted preset came back");
  c.destroy();
});

test("state autosave restores the last patch", async () => {
  const idb = fakeIndexedDB();
  const a = await newController();
  a.tick();
  a.setParam(CUTOFF, 640);
  a.tick();
  const sa = new StateAutosave({ controller: a, indexedDB: idb, dbId: DB_ID });
  await sa.open();
  sa.schedule();
  await sa.flush();
  await sa.drain();
  a.destroy();

  const b = await newController();
  const sb = new StateAutosave({ controller: b, indexedDB: idb, dbId: DB_ID });
  await sb.restore();
  const p = b.tick().find((e) => e.kind === "param_changed" && e.id === CUTOFF);
  assert.ok(p, "restore produced no re-broadcast");
  assert.ok(Math.abs(p.plain - 640) < 1, `patch not restored: ${p.plain}`);
  b.destroy();
});

test("a share link wins over the autosaved session", async () => {
  const idb = fakeIndexedDB();
  // Autosave one patch…
  const a = await newController();
  a.tick();
  a.setParam(CUTOFF, 300);
  a.tick();
  const sa = new StateAutosave({ controller: a, indexedDB: idb, dbId: DB_ID });
  await sa.open();
  sa.schedule();
  await sa.flush();
  await sa.drain();
  // …and build a share link carrying a different one.
  a.setParam(CUTOFF, 5000);
  a.tick();
  const link = patchIo.shareLinkFor(a, { origin: "https://x", pathname: "/" });
  a.destroy();

  const b = await newController();
  const hash = link.slice(link.indexOf("#"));
  const fromShare = patchIo.applyShareLinkOnBoot(b, {
    location: { hash },
    history: { replaceState() {} },
  });
  assert.equal(fromShare, true, "the share link was not applied");
  const p = b.tick().find((e) => e.kind === "param_changed" && e.id === CUTOFF);
  assert.ok(Math.abs(p.plain - 5000) < 10, `share link lost: ${p.plain}`);
  b.destroy();
});

test("an exported patch re-imports to the same sound", async () => {
  const a = await newController();
  a.tick();
  a.setParam(CUTOFF, 1234);
  a.tick();
  const toml = a.exportToml("Exported");
  a.setParam(CUTOFF, 200);
  a.tick();
  assert.equal(a.importToml(toml), true);
  const p = a.tick().find((e) => e.kind === "param_changed" && e.id === CUTOFF);
  assert.ok(Math.abs(p.plain - 1234) < 1);
  a.destroy();
});

test("no IndexedDB leaves a playable instrument, and does not throw", async () => {
  // Private mode / blocked storage: hydrate must resolve false, publish an empty
  // corpus, and let the synth come up at defaults.
  const c = await newController();
  const broken = {
    open: () => {
      const req = { onerror: null, onsuccess: null, onupgradeneeded: null };
      queueMicrotask(() => req.onerror && req.onerror({ target: { error: new Error("blocked") } }));
      return req;
    },
  };
  const p = new PresetPersistence({ controller: c, indexedDB: broken, dbId: DB_ID });
  const ok = await p.hydrate();
  assert.equal(ok, false, "hydrate must report unavailable, not throw");
  // Still drains, so the wasm journal cannot grow unbounded with no storage.
  c.tick();
  c.savePreset("Orphan", null);
  c.tick();
  await p.flush();
  assert.deepEqual(c.takeJournal(), [], "journal not drained when storage is absent");
  c.destroy();
});

test("boot hydrates BEFORE the queued ready, so the restored patch is painted", async () => {
  // The ordering the whole ticket turns on. `ready` sits in the boot queue and
  // triggers the re-broadcast that paints every control; if hydration ran after
  // install(), the page would paint defaults and then disagree with the model.
  if (!engineWasm) throw new Error("engine wasm not built — build it, do not skip");
  const idb = fakeIndexedDB();

  // Seed the store with an autosaved patch from a previous "session".
  const seed = await newController();
  seed.tick();
  seed.setParam(CUTOFF, 987);
  seed.tick();
  const ss = new StateAutosave({ controller: seed, indexedDB: idb, dbId: DB_ID });
  await ss.open();
  ss.schedule();
  await ss.flush();
  await ss.drain();
  seed.destroy();

  const batches = [];
  const win = {
    __VXN_UI_QUEUE__: [JSON.stringify({ op: "ready" })],
    __vxn: { applyViewEvents: (a) => batches.push(a), applyPresetCorpus: () => {} },
    location: { hash: "" },
    history: { replaceState() {} },
  };
  const fetchImpl = async (url) => ({
    ok: true,
    arrayBuffer: async () => (String(url).includes("controller") ? wasmBytes : engineWasm),
  });

  const { bridge, controller, autosave } = await boot({
    win,
    fetchImpl,
    autoGesture: false,
    autoInputs: false,
    adapters: {
      PresetPersistence: class extends PresetPersistence {
        constructor(o) {
          super({ ...o, indexedDB: idb });
        }
      },
      StateAutosave: class extends StateAutosave {
        constructor(o) {
          super({ ...o, indexedDB: idb });
        }
      },
      patchIo,
    },
  });
  bridge.pump();
  bridge.stop();
  // Settle the debounced autosave: a timer firing after destroy() would reach
  // into freed wasm memory and trap.
  if (autosave) {
    await autosave.flush();
    await autosave.drain();
  }

  const painted = batches
    .flat()
    .filter((e) => e.kind === "param_changed" && e.id === CUTOFF)
    .pop();
  assert.ok(painted, "the queued ready did not paint");
  assert.ok(
    Math.abs(painted.plain - 987) < 1,
    `the page painted ${painted.plain}, not the restored 987 — hydration ran too late`,
  );
  controller.destroy();
});
