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
