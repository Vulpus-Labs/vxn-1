---
id: "0045"
product: vxn-2
title: "COOP/COEP dev server + production hosting doc"
priority: medium
created: 2026-06-15
epic: E016
depends: ["0041"]
---

## Summary

The cross-origin-isolation serving story for both dev and prod. `SharedArrayBuffer`
(the whole E015 transport) requires `crossOriginIsolated`, which requires
`Cross-Origin-Opener-Policy: same-origin` + `Cross-Origin-Embedder-Policy:
require-corp`. Promote the spike's `serve-coep.mjs` into the real dev server and
document the production recipe.

## Design

- **Dev server.** Promote
  [serve-coep.mjs](../../vxn-1/crates/vxn-wasm/serve-coep.mjs) (already sets both
  headers, already proven for the 0035 spike) into the real dev server, serving
  the 0041 dist/. Wire it as `cargo xtask web --serve` so one command builds +
  serves with isolation on.
- **Verify isolation.** On the served page, `self.crossOriginIsolated === true`
  and `SharedArrayBuffer` is constructible — the precondition for 0042's SABs.
- **Production hosting doc.** A doc (`docs/web-hosting.md` or in the crate
  README) giving the same two headers on a static host / CDN, the CORP
  implications for any cross-origin assets (require-corp means every subresource
  must be same-origin or carry CORP/CORS — the spike keeps everything
  same-origin), and the embedding caveat (COOP/COEP can break iframing + third-
  party assets — call it out per the epic risk).
- **Validate on a real host.** Deploy the dist/ to at least one static host
  (e.g. a headers-capable static host / Netlify `_headers` / Cloudflare) and
  confirm `crossOriginIsolated` true + SAB live there, not just locally.

## Acceptance criteria

- [ ] `cargo xtask web --serve` builds and serves the dist/ with COOP/COEP set;
      the page reports `crossOriginIsolated === true` and SAB constructs.
- [ ] A production hosting doc specifies the headers, the CORP/CORS implications
      for cross-origin assets, and the iframe/embedding caveat.
- [ ] The recipe is validated on at least one real static host (not just the
      local dev server), with isolation confirmed there.

## Notes

- Depends on [0041](0041-web-xtask-build-bundle.md) for the dist/ it serves; the
  header mechanism is already proven by the 0035 spike's `serve-coep.mjs` and
  `SPIKE-0035-findings.md`.
- Out of scope: CI deploy automation (E020); cross-browser SAB/isolation
  coverage matrix (E020).

## Close-out (2026-06-15)

(Frontmatter says `product: vxn-2` but the work is all in `vxn-1` — the web build
lives there; treat the label as a mislabel.)

- **`cargo xtask web --serve [--port N]`** added —
  [main.rs](../../vxn-1/xtask/src/main.rs): new `--serve`/`--port` parsing
  (`arg_value` helper), `serve_dist()` hands `target/web-dist/` to
  [serve-coep.mjs](../../vxn-1/crates/vxn-wasm/serve-coep.mjs) (already promoted
  from the 0035 spike). Verified live: `curl -I` on the served document and the
  `.wasm` subresource both return COOP `same-origin` + COEP `require-corp` (+CORP
  `same-origin`), wasm as `application/wasm` — the precondition for
  `crossOriginIsolated === true` + constructible SAB (flag proven to flip under
  exactly these headers in SPIKE-0035-findings.md; the booting `index.html`
  reports it inline).
- **Production hosting doc** —
  [WEB-HOSTING.md](../../vxn-1/crates/vxn-wasm/WEB-HOSTING.md): the two headers,
  the `require-corp` CORP/CORS implication for cross-origin subresources, the
  COOP iframe/popup embedding caveat, and per-host recipes (Netlify/CF Pages
  `_headers`, nginx, Caddy, S3+CloudFront). Linked from the crate
  [README.md](../../vxn-1/crates/vxn-wasm/README.md). `cargo xtask web` also
  emits a Netlify `_headers` into the bundle (`web_dist_headers()` in main.rs).
- **Validated on a real static host** — deployed the bundle to
  `vulpuslabs.com/products/vxn-1/web/` (Netlify, separate `vulpus-labs-site`
  repo) via the new
  [deploy-web.sh](../../vxn-1/crates/vxn-wasm/deploy-web.sh) (build → copy →
  scoped root `_headers` → commit → push). Confirmed live: `curl -sI` shows all
  three isolation headers on both the document and `vxn_wasm.wasm` at the real
  origin, not just localhost.
