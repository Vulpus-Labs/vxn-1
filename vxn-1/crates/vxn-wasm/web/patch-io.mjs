// Patch export / import + URL share-link (E019 / 0066).
//
// The fifth and final E019 capability: share a patch OFF-device. Two channels,
// both reusing the controller's existing byte surfaces:
//
//   - FILE  export/import a `.toml` — the desktop-compatible name-keyed format
//     (`vxn_app::preset_toml`, byte-identical across native + wasm). Export
//     downloads a Blob; import reads a picked file's text and applies it through
//     `controller.importToml` (the same model-restore path a preset load uses),
//     then re-broadcasts `EditorReady` so the UI + param SAB reseed.
//   - URL   a `#patch=…` share-link encoding the COMPACT binary state blob
//     (0065's snapshot) as base64url, kept in the fragment so it is never sent to
//     a server. On boot, a present fragment is decoded + applied BEFORE the
//     EditorReady re-broadcast (same ordering as 0065's restore), then stripped
//     from the URL so a reload doesn't re-import a stale patch over later edits.
//
// The pure codec (base64url, fragment parse/build, size cap) has NO DOM/wasm
// dependency so the Node test exercises it directly; the DOM-touching export/
// import/share helpers take seams (doc / location / url) for the same reason.

// The fragment key: `#patch=…`. Kept out of the query string so it isn't sent to
// any server (fragments are client-only).
const FRAGMENT_KEY = "patch";

// Conservative cap on the encoded patch in a URL fragment. The binary state blob
// is ~670 bytes → ~893 base64url chars, far under this; the cap guards a future
// larger format from producing an impractical URL (share then falls back to
// file-only, surfaced by the caller).
export const MAX_SHARE_FRAGMENT_LEN = 8192;

// ---- base64url codec (RFC 4648 §5: -/_ , no padding) ------------------------
//
// Works in both the browser (btoa/atob) and Node (Buffer) so the one code path
// is what the Node test runs.

export function bytesToBase64url(bytes) {
  const u8 = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  let b64;
  if (typeof btoa === "function") {
    let bin = "";
    for (let i = 0; i < u8.length; i++) bin += String.fromCharCode(u8[i]);
    b64 = btoa(bin);
  } else {
    b64 = Buffer.from(u8).toString("base64");
  }
  return b64.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function base64urlToBytes(s) {
  const b64 = s.replace(/-/g, "+").replace(/_/g, "/");
  const padded = b64.length % 4 === 0 ? b64 : b64 + "=".repeat(4 - (b64.length % 4));
  if (typeof atob === "function") {
    const bin = atob(padded); // throws on invalid input — caller catches
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }
  return new Uint8Array(Buffer.from(padded, "base64"));
}

// ---- fragment parse / build (pure) ------------------------------------------

// Pull the encoded patch value out of a `location.hash` string ("#patch=…",
// possibly alongside other `&`-joined fragment params). Returns the raw encoded
// value, or null if absent.
export function patchParamFromHash(hash) {
  if (!hash) return null;
  const h = hash.charAt(0) === "#" ? hash.slice(1) : hash;
  for (const part of h.split("&")) {
    const eq = part.indexOf("=");
    if (eq < 0) continue;
    if (part.slice(0, eq) === FRAGMENT_KEY) {
      try {
        return decodeURIComponent(part.slice(eq + 1));
      } catch {
        return part.slice(eq + 1);
      }
    }
  }
  return null;
}

// Decode a share-fragment value to the binary patch blob, or null if malformed
// (bad base64url / empty). Never throws.
export function decodeShareFragment(value) {
  if (!value) return null;
  try {
    const bytes = base64urlToBytes(value);
    return bytes.length ? bytes : null;
  } catch {
    return null;
  }
}

// Build a `#patch=…` share-link for a binary patch blob. Returns the full URL, or
// null if the encoded blob exceeds the practical fragment cap (caller falls back
// to file-only).
export function buildShareUrl(blob, { origin = "", pathname = "" } = {}) {
  const enc = bytesToBase64url(blob);
  if (enc.length > MAX_SHARE_FRAGMENT_LEN) return null;
  return `${origin}${pathname}#${FRAGMENT_KEY}=${enc}`;
}

// ---- controller-coupled helpers ---------------------------------------------

// Snapshot the current patch and build a share-link from it (compact binary blob,
// base64url'd into the fragment). Returns the URL, or null if too big to share by
// URL (the caller surfaces "use file export instead").
export function shareLinkFor(controller, loc = globalThis.location) {
  const blob = controller.snapshotState();
  return buildShareUrl(blob, { origin: loc.origin, pathname: loc.pathname });
}

// Apply a `#patch=…` fragment present at boot into the model BEFORE the
// EditorReady re-broadcast. Returns true if a patch was applied. Best-effort: a
// missing / malformed fragment returns false (boot continues to autosave restore
// / defaults). On success the fragment is stripped from the URL via replaceState
// so a later reload doesn't re-import the shared patch over the user's edits.
export function applyShareLinkOnBoot(
  controller,
  { location = globalThis.location, history = globalThis.history } = {},
) {
  const blob = decodeShareFragment(patchParamFromHash(location.hash));
  if (!blob) return false;
  const ok = controller.restoreState(blob);
  if (ok && history && typeof history.replaceState === "function") {
    try {
      history.replaceState(null, "", (location.pathname || "") + (location.search || ""));
    } catch {
      /* non-fatal: the patch still applied, the URL just keeps its fragment */
    }
  }
  return ok;
}

// Strip characters illegal in filenames; fall back to a default if empty.
export function sanitizeFilename(name) {
  const s = (name || "").trim().replace(/[\/\\:*?"<>|\x00-\x1f]/g, "_");
  return s || "VXN1 Patch";
}

// Export the current patch as a downloadable `.toml` (the desktop-compatible
// name-keyed format). Builds the TOML via the controller, wraps it in a Blob, and
// clicks a transient anchor. Returns the TOML text. DOM seams for testing.
export function exportPatchFile(
  controller,
  { name = "VXN1 Patch", doc = globalThis.document, url = globalThis.URL } = {},
) {
  const toml = controller.exportToml(name);
  const blob = new Blob([toml], { type: "application/toml" });
  const href = url.createObjectURL(blob);
  const a = doc.createElement("a");
  a.href = href;
  a.download = `${sanitizeFilename(name)}.toml`;
  doc.body.appendChild(a);
  a.click();
  doc.body.removeChild(a);
  // Revoke after a tick so the download has started.
  setTimeout(() => {
    try {
      url.revokeObjectURL(href);
    } catch {
      /* ignore */
    }
  }, 0);
  return toml;
}

// Open a file picker and import the chosen `.toml` into the model. On success
// re-broadcasts EditorReady (UI + param SAB reseed) and reports via
// onResult({ ok, name, error }). A malformed file reports ok:false with a
// user-visible message — never throws past the seam. DOM seams for testing.
export function importPatchFile(
  controller,
  { doc = globalThis.document, onResult = () => {} } = {},
) {
  const input = doc.createElement("input");
  input.type = "file";
  input.accept = ".toml,application/toml,text/plain";
  input.style.display = "none";
  const cleanup = () => {
    try {
      input.remove();
    } catch {
      /* ignore */
    }
  };
  input.addEventListener("change", async () => {
    const file = input.files && input.files[0];
    cleanup();
    if (!file) {
      onResult({ ok: false, error: "no file selected" });
      return;
    }
    try {
      const text = await file.text();
      const ok = controller.importToml(text);
      if (ok) controller.editorReady();
      onResult({
        ok,
        name: file.name,
        error: ok ? null : "not a valid VXN1 patch",
      });
    } catch (e) {
      onResult({ ok: false, name: file.name, error: (e && e.message) || "read failed" });
    }
  });
  doc.body.appendChild(input);
  input.click();
}
