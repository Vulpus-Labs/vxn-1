// Minimal static server that sets the cross-origin-isolation headers required
// for SharedArrayBuffer (ticket 0035). Serves the web/ directory.
//
//   node serve-coep.mjs [port]      (default 8080)
//   open http://localhost:8080/index-0035.html
//
// The two headers below are the WHOLE isolation story for SAB:
//   Cross-Origin-Opener-Policy:   same-origin
//   Cross-Origin-Embedder-Policy: require-corp
// With both present on the top-level document, the browser sets
// `self.crossOriginIsolated === true` and SharedArrayBuffer becomes
// constructible. require-corp additionally means every subresource (the .wasm,
// the .mjs modules) must be same-origin or carry CORP/CORS — here everything is
// same-origin so it just works. This is the exact config a real deploy needs
// (E016); documented in SPIKE-0035-findings.md.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { extname, join, normalize } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(fileURLToPath(new URL(".", import.meta.url)), "web");
const PORT = Number(process.argv[2] ?? 8080);

const MIME = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".wasm": "application/wasm",
  ".css": "text/css; charset=utf-8",
};

createServer(async (req, res) => {
  // The isolation headers — set on every response so subresources qualify too.
  res.setHeader("Cross-Origin-Opener-Policy", "same-origin");
  res.setHeader("Cross-Origin-Embedder-Policy", "require-corp");
  res.setHeader("Cross-Origin-Resource-Policy", "same-origin");

  let path = decodeURIComponent(new URL(req.url, "http://x").pathname);
  if (path === "/") path = "/index-0035.html";
  const file = normalize(join(ROOT, path));
  if (!file.startsWith(ROOT)) {
    res.writeHead(403).end("forbidden");
    return;
  }
  try {
    const body = await readFile(file);
    res.writeHead(200, { "Content-Type": MIME[extname(file)] ?? "application/octet-stream" });
    res.end(body);
  } catch {
    res.writeHead(404).end("not found");
  }
}).listen(PORT, () => {
  console.log(`serving ${ROOT} on http://localhost:${PORT} with COOP/COEP`);
  console.log(`open  http://localhost:${PORT}/index-0035.html`);
});
