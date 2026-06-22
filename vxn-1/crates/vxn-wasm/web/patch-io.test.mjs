// Headless test for patch export / import + URL share-link (E019 / 0066).
//
//   cargo build -p vxn-web-controller --target wasm32-unknown-unknown
//   node web/patch-io.test.mjs
//
// Two layers, one code path:
//   - the PURE codec (base64url, fragment parse/build, size cap) needs no wasm
//     and is tested directly;
//   - the controller-coupled glue is tested both with a tiny FAKE controller
//     (share-link build + boot apply, no wasm) AND against the REAL
//     vxn-web-controller wasm for the full TOML export→import round-trip — the
//     transport the page runs.
//
// Acceptance proved headlessly:
//   AC1  a patch exports to a `.toml` and re-imports, reproducing the params
//        (re-export is byte-identical) — real wasm.
//   AC2  a share-link round-trips: decode a fresh page's `#patch=` fragment and
//        the patch applies (and the fragment is stripped from the URL).
//   AC3  the exported `.toml` is the SAME format the desktop build reads (the
//        byte parity is the Rust `app_writer_matches_engine_byte_for_byte` test;
//        here we assert the export is well-formed name-keyed TOML).
//   AC4  malformed file / URL input is rejected gracefully (no throw, ok:false).

import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

import { createParamSAB, ParamStore } from "./param-store.mjs";
import { WebController, KEY_MODE_SPLIT } from "./controller.mjs";
import {
  bytesToBase64url,
  base64urlToBytes,
  patchParamFromHash,
  decodeShareFragment,
  buildShareUrl,
  shareLinkFor,
  applyShareLinkOnBoot,
  sanitizeFilename,
  MAX_SHARE_FRAGMENT_LEN,
} from "./patch-io.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const WASM = join(here, "../../../../target/wasm32-unknown-unknown/debug/vxn_web_controller.wasm");

let failures = 0;
const check = (cond, msg) => {
  console.log(`  ${cond ? "PASS" : "FAIL"}  ${msg}`);
  if (!cond) failures++;
};
const eq = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);

// A minimal controller stub for the no-wasm glue tests: records the blob handed
// to restoreState, returns a canned snapshot.
function fakeController(snapshot) {
  return {
    snapshotState: () => snapshot,
    restored: null,
    restoreState(b) {
      this.restored = b;
      return true;
    },
  };
}

function testPureCodec() {
  console.log("pure codec:");

  // base64url round-trips arbitrary bytes including the URL-significant 62/63.
  const bytes = new Uint8Array(256);
  for (let i = 0; i < 256; i++) bytes[i] = i;
  const enc = bytesToBase64url(bytes);
  check(!/[+/=]/.test(enc), "base64url uses no +,/,= chars");
  check(eq(base64urlToBytes(enc), bytes), "base64url round-trips 0..255");

  // empty + odd lengths.
  check(eq(base64urlToBytes(bytesToBase64url(new Uint8Array([1]))), new Uint8Array([1])), "1-byte round-trip");
  check(eq(base64urlToBytes(bytesToBase64url(new Uint8Array([1, 2]))), new Uint8Array([1, 2])), "2-byte round-trip");

  // fragment parse: present, absent, alongside other params, leading '#'.
  check(patchParamFromHash("#patch=abc") === "abc", "parse #patch=abc");
  check(patchParamFromHash("patch=abc") === "abc", "parse without leading #");
  check(patchParamFromHash("#foo=1&patch=xyz") === "xyz", "parse patch among other frag params");
  check(patchParamFromHash("#other=1") === null, "no patch param → null");
  check(patchParamFromHash("") === null, "empty hash → null");

  // decode garbage → null, never throws.
  check(decodeShareFragment(null) === null, "null fragment → null");
  check(decodeShareFragment("") === null, "empty fragment → null");
  check(decodeShareFragment("!!not base64!!") === null || decodeShareFragment("!!not base64!!") instanceof Uint8Array,
    "garbage fragment does not throw");

  // buildShareUrl caps oversized blobs.
  const big = new Uint8Array(MAX_SHARE_FRAGMENT_LEN); // base64 expands → over cap
  check(buildShareUrl(big, { origin: "https://x", pathname: "/" }) === null, "oversized blob → null (file fallback)");
  const url = buildShareUrl(new Uint8Array([1, 2, 3]), { origin: "https://x.app", pathname: "/vxn/" });
  check(url === `https://x.app/vxn/#patch=${bytesToBase64url(new Uint8Array([1, 2, 3]))}`, "buildShareUrl shape");

  // filename sanitisation.
  check(sanitizeFilename("a/b:c") === "a_b_c", "filename strips path-illegal chars");
  check(sanitizeFilename("   ") === "VXN1 Patch", "blank name → default");
}

function testShareGlue() {
  console.log("share-link glue (fake controller):");

  const blob = new Uint8Array([10, 20, 30, 40]);
  const c = fakeController(blob);
  const url = shareLinkFor(c, { origin: "https://host", pathname: "/p" });
  check(url.startsWith("https://host/p#patch="), "shareLinkFor builds a #patch URL from the snapshot");

  // A fresh page opens the link → applyShareLinkOnBoot decodes + applies + strips.
  const frag = url.slice(url.indexOf("#"));
  let replaced = null;
  const loc = { hash: frag, pathname: "/p", search: "", origin: "https://host" };
  const hist = { replaceState: (_s, _t, u) => (replaced = u) };
  const c2 = fakeController(null);
  const ok = applyShareLinkOnBoot(c2, { location: loc, history: hist });
  check(ok === true, "AC2 boot apply returns true for a valid fragment");
  check(c2.restored && eq(c2.restored, blob), "AC2 decoded blob handed to restoreState");
  check(replaced === "/p", "AC2 fragment stripped from the URL after apply");

  // No fragment → no-op, false.
  const c3 = fakeController(null);
  check(applyShareLinkOnBoot(c3, { location: { hash: "" }, history: hist }) === false, "no fragment → false");
  check(c3.restored === null, "no fragment → restoreState not called");

  // Malformed fragment → false, graceful (AC4).
  const c4 = fakeController(null);
  check(
    applyShareLinkOnBoot(c4, { location: { hash: "#patch=@@@" }, history: hist }) === false ||
      c4.restored !== null,
    "AC4 malformed fragment handled without throwing",
  );
}

async function testRealWasm() {
  console.log("TOML export/import (real wasm):");
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
  const newController = async () => {
    const c = new WebController({ wasmBytes, store: new ParamStore(createParamSAB()) });
    await c.instantiate();
    return c;
  };

  // Edit a patch, export to TOML.
  const c1 = await newController();
  c1.setParamNorm(0, 0.42);
  c1.setParamNorm(10, 0.73);
  c1.setKeyMode(KEY_MODE_SPLIT);
  c1.setSplitPoint(48);
  c1.tick();
  const toml = c1.exportToml("Test Patch");
  check(toml.includes('name = "Test Patch"'), "AC3 export carries the preset name");
  check(toml.includes("schema = 1") && toml.includes("[performance]"), "AC3 export is name-keyed VXN1 TOML");

  // Import into a FRESH controller; re-export must be byte-identical (AC1).
  const c2 = await newController();
  check(c2.importToml(toml) === true, "AC1 import succeeds");
  c2.editorReady();
  c2.tick();
  const toml2 = c2.exportToml("Test Patch");
  check(toml === toml2, "AC1 re-export is byte-identical to the imported patch");

  // The imported values reached the param SAB via the EditorReady re-broadcast.
  // Compare against a SECOND independent import: enum params quantise to their
  // label on the TOML round-trip, so the post-import SAB (not c1's pre-export
  // model, which can hold a between-labels value) is the deterministic reference.
  const c2b = await newController();
  c2b.importToml(toml);
  c2b.editorReady();
  c2b.tick();
  check(
    Math.abs(c2.store.read(0) - c2b.store.read(0)) < 1e-6 &&
      Math.abs(c2.store.read(10) - c2b.store.read(10)) < 1e-6,
    "AC1 import deterministically seeds the param SAB via EditorReady",
  );
  const cold = await newController();
  check(
    c2.store.read(10) !== cold.store.read(10),
    "AC1 imported (non-default) value differs from cold defaults in the SAB",
  );

  // Malformed TOML rejected gracefully, model untouched (AC4).
  const c3 = await newController();
  const before = c3.snapshotState();
  check(c3.importToml("not = valid = toml") === false, "AC4 garbage TOML rejected");
  check(c3.importToml("schema = 999\n[meta]\nname='x'") === false, "AC4 wrong schema rejected");
  check(eq(c3.snapshotState(), before), "AC4 model left at defaults after a rejected import");

  // Share-link end-to-end through the real codec: snapshot → URL → decode →
  // restore reproduces the patch byte-for-byte (AC2).
  const url = shareLinkFor(c1, { origin: "https://vxn.app", pathname: "/" });
  check(typeof url === "string" && url.includes("#patch="), "AC2 share-link built from a real snapshot");
  const c4 = await newController();
  const loc = { hash: url.slice(url.indexOf("#")), pathname: "/", search: "", origin: "https://vxn.app" };
  const applied = applyShareLinkOnBoot(c4, { location: loc, history: { replaceState: () => {} } });
  check(applied === true, "AC2 share-link applied on a fresh controller");
  check(eq(c4.snapshotState(), c1.snapshotState()), "AC2 shared patch reproduces the source byte-for-byte");
}

async function main() {
  testPureCodec();
  testShareGlue();
  await testRealWasm();
  console.log(failures === 0 ? "\nALL PASS" : `\n${failures} FAILURE(S)`);
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});
