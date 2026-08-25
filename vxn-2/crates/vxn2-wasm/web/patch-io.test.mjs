// Patch export / import + URL share-link (0159). Run after `cargo xtask web`:
//   node --test crates/vxn2-wasm/web/patch-io.test.mjs
//
// Two layers, one code path:
//   - the PURE codec (base64url, fragment parse/build, size cap) needs no wasm
//     and is tested directly;
//   - the controller-coupled glue is tested both with a tiny FAKE controller
//     (share-link build + boot apply) AND against the REAL vxn2-web-controller
//     wasm for the full TOML export→import round-trip + share-link — the
//     transport the page runs.
// Real-wasm cases skip (not fail) when the web bundle isn't built.

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { WebController } from "./controller.mjs";
import { PRODUCT } from "./faceplate-bridge.mjs";
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
} from "../../../../crates/vxn-core-web/assets/patch-io.mjs";

const WASM = fileURLToPath(new URL("../../../../target/web-dist/vxn2_web_controller.wasm", import.meta.url));
const HAVE = existsSync(WASM);
const wasmBytes = HAVE ? readFileSync(WASM) : null;

const eq = (a, b) => a.length === b.length && a.every((v, i) => v === b[i]);

// A minimal controller stub for the no-wasm glue tests.
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

test("pure codec: base64url + fragment parse/build round-trip", () => {
  const bytes = new Uint8Array(256);
  for (let i = 0; i < 256; i++) bytes[i] = i;
  const enc = bytesToBase64url(bytes);
  assert.ok(!/[+/=]/.test(enc), "base64url uses no +,/,= chars");
  assert.ok(eq(base64urlToBytes(enc), bytes), "base64url round-trips 0..255");
  assert.ok(eq(base64urlToBytes(bytesToBase64url(new Uint8Array([1]))), new Uint8Array([1])));
  assert.ok(eq(base64urlToBytes(bytesToBase64url(new Uint8Array([1, 2]))), new Uint8Array([1, 2])));

  assert.equal(patchParamFromHash("#patch=abc"), "abc");
  assert.equal(patchParamFromHash("patch=abc"), "abc");
  assert.equal(patchParamFromHash("#foo=1&patch=xyz"), "xyz");
  assert.equal(patchParamFromHash("#other=1"), null);
  assert.equal(patchParamFromHash(""), null);

  assert.equal(decodeShareFragment(null), null);
  assert.equal(decodeShareFragment(""), null);
  assert.doesNotThrow(() => decodeShareFragment("!!not base64!!"));

  const big = new Uint8Array(MAX_SHARE_FRAGMENT_LEN); // base64 expands → over cap
  assert.equal(buildShareUrl(big, { origin: "https://x", pathname: "/" }), null);
  const url = buildShareUrl(new Uint8Array([1, 2, 3]), { origin: "https://x.app", pathname: "/vxn/" });
  assert.equal(url, `https://x.app/vxn/#patch=${bytesToBase64url(new Uint8Array([1, 2, 3]))}`);

  assert.equal(sanitizeFilename("a/b:c"), "a_b_c");
  assert.equal(sanitizeFilename("   ", `${PRODUCT} Patch`), `${PRODUCT} Patch`);
});

test("share-link glue (fake controller): build → boot apply → strip", () => {
  const blob = new Uint8Array([10, 20, 30, 40]);
  const c = fakeController(blob);
  const url = shareLinkFor(c, { origin: "https://host", pathname: "/p" });
  assert.ok(url.startsWith("https://host/p#patch="));

  const frag = url.slice(url.indexOf("#"));
  let replaced = null;
  const loc = { hash: frag, pathname: "/p", search: "", origin: "https://host" };
  const hist = { replaceState: (_s, _t, u) => (replaced = u) };
  const c2 = fakeController(null);
  assert.equal(applyShareLinkOnBoot(c2, { location: loc, history: hist }), true);
  assert.ok(c2.restored && eq(c2.restored, blob), "decoded blob handed to restoreState");
  assert.equal(replaced, "/p", "fragment stripped from the URL after apply");

  const c3 = fakeController(null);
  assert.equal(applyShareLinkOnBoot(c3, { location: { hash: "" }, history: hist }), false);
  assert.equal(c3.restored, null, "no fragment → restoreState not called");

  const c4 = fakeController(null);
  assert.doesNotThrow(() =>
    applyShareLinkOnBoot(c4, { location: { hash: "#patch=@@@" }, history: hist }),
  );
});

test("TOML export→import round-trips through the real wasm", { skip: !HAVE }, async () => {
  const newController = async () => {
    const c = new WebController({ wasmBytes });
    await c.instantiate();
    return c;
  };

  const c1 = await newController();
  c1.setParamNorm(0, 0.42);
  c1.setParamNorm(10, 0.73);
  c1.tick();
  const toml = c1.exportToml("Test Patch");
  assert.ok(toml.includes('name = "Test Patch"'), "export carries the preset name");
  assert.ok(toml.includes("schema = 1"), "export is name-keyed VXN2 TOML");

  // Import into a FRESH controller; re-export must be byte-identical.
  const c2 = await newController();
  assert.equal(c2.importToml(toml), true, "import succeeds");
  c2.editorReady();
  c2.tick();
  assert.equal(c2.exportToml("Test Patch"), toml, "re-export is byte-identical to the imported patch");

  // Malformed TOML rejected gracefully, model untouched.
  const c3 = await newController();
  const before = c3.snapshotState();
  assert.equal(c3.importToml("not = valid = toml"), false, "garbage TOML rejected");
  assert.equal(c3.importToml("schema = 999\n[meta]\nname='x'"), false, "wrong schema rejected");
  assert.ok(eq(c3.snapshotState(), before), "model left at defaults after a rejected import");
});

test("share-link end-to-end reproduces the patch byte-for-byte (real wasm)", { skip: !HAVE }, async () => {
  const newController = async () => {
    const c = new WebController({ wasmBytes });
    await c.instantiate();
    return c;
  };
  const c1 = await newController();
  c1.setParamNorm(0, 0.42);
  c1.setParamNorm(10, 0.73);
  c1.tick();
  const url = shareLinkFor(c1, { origin: "https://vxn.app", pathname: "/" });
  assert.ok(typeof url === "string" && url.includes("#patch="), "share-link built from a real snapshot");

  const c4 = await newController();
  const loc = { hash: url.slice(url.indexOf("#")), pathname: "/", search: "", origin: "https://vxn.app" };
  assert.equal(
    applyShareLinkOnBoot(c4, { location: loc, history: { replaceState: () => {} } }),
    true,
    "share-link applied on a fresh controller",
  );
  assert.ok(eq(c4.snapshotState(), c1.snapshotState()), "shared patch reproduces the source byte-for-byte");
});
