# Hosting VXN1 on the web (ticket 0045)

The web build (`cargo xtask web`) ships a `SharedArrayBuffer`-backed transport
(the E015 event-ring + param-store). `SharedArrayBuffer` is only constructible
when the page is **cross-origin isolated**, and a page is cross-origin isolated
only when the top-level document is served with **both** of these response
headers:

```
Cross-Origin-Opener-Policy:   same-origin
Cross-Origin-Embedder-Policy: require-corp
```

With both present the browser sets `self.crossOriginIsolated === true` and
`new SharedArrayBuffer(n)` works. Without them, `crossOriginIsolated` is `false`,
the SABs fail to construct, and the synth cannot boot. **These two headers are
the entire isolation story** — there is nothing else to configure.

## Verify it worked

Open the served page and check (the bundled `index.html` already reports this
inline, or in the devtools console):

```js
self.crossOriginIsolated          // must be true
new SharedArrayBuffer(8)          // must not throw
```

If `crossOriginIsolated` is `false`, the headers are missing or misspelled on
the **document** response — check them with `curl -I <url>`, not just the
browser, since a service worker or CDN can strip them.

## Dev server

```bash
cargo xtask web --serve            # build + serve on http://localhost:8080
cargo xtask web --serve --port 9000
```

This builds `target/web-dist/` and hands it to
[`serve-coep.mjs`](serve-coep.mjs) (needs `node` on PATH), which sets the two
headers (plus `Cross-Origin-Resource-Policy: same-origin`) on **every** response
so subresources qualify too. It is the local mirror of the production recipe
below.

## Production recipe

The headers must come from the **host/CDN edge** — they are response headers, not
anything bakeable into the static files themselves. `cargo xtask web` drops a
Netlify/Cloudflare-Pages `_headers` file into `target/web-dist/` so the simplest
deploys carry them with zero extra config; other hosts need their own header
config.

### Netlify / Cloudflare Pages (`_headers`, emitted into dist)

```
/*
  Cross-Origin-Opener-Policy: same-origin
  Cross-Origin-Embedder-Policy: require-corp
  Cross-Origin-Resource-Policy: same-origin
```

Drop `target/web-dist/` onto either host as-is — the `_headers` file is read
automatically. (Netlify also accepts the equivalent `[[headers]]` block in
`netlify.toml`.)

### nginx

```nginx
location / {
    add_header Cross-Origin-Opener-Policy   "same-origin"   always;
    add_header Cross-Origin-Embedder-Policy "require-corp"  always;
    add_header Cross-Origin-Resource-Policy "same-origin"   always;
    types { application/wasm wasm; }   # ensure .wasm → application/wasm
}
```

### Caddy

```
header {
    Cross-Origin-Opener-Policy   "same-origin"
    Cross-Origin-Embedder-Policy "require-corp"
    Cross-Origin-Resource-Policy "same-origin"
}
```

### S3 + CloudFront

S3 object metadata cannot set these reliably; attach a CloudFront
**response-headers policy** (or a viewer-response function) adding COOP and COEP
to all responses. Also confirm `.wasm` is served as `application/wasm`.

## `require-corp`: the cross-origin-asset implication

`Cross-Origin-Embedder-Policy: require-corp` means **every subresource the page
loads** — scripts, the `.wasm`, images, fonts, audio — must be either:

- **same-origin** (the default for this bundle — everything is served from the
  one origin, so it "just works"), or
- cross-origin **and** explicitly opted in, by carrying either
  `Cross-Origin-Resource-Policy: cross-origin` (or `same-site`) **or** valid
  CORS (`Access-Control-Allow-Origin` + a `crossorigin` attribute on the tag).

A cross-origin asset that carries neither will be **blocked** under
`require-corp`. So: keep all assets same-origin (the bundle does), or ensure any
third-party asset you add sets CORP/CORS. There is no third way.

## Embedding caveat

`Cross-Origin-Opener-Policy: same-origin` severs the `window.opener` relationship
with cross-origin openers, and `require-corp` restricts what can be embedded. Two
consequences worth flagging:

- **Embedding VXN1 in a cross-origin `<iframe>`** (e.g. on someone else's site)
  requires that parent page to *also* be cross-origin isolated and to allow the
  embed; otherwise the SABs won't construct inside the frame.
- **Cross-origin popups / OAuth-style window handoffs** initiated from the page
  lose the `opener` link under COOP `same-origin`. If you need those, you need a
  different (weaker) COOP value — which then forfeits isolation, and with it
  `SharedArrayBuffer`. The two requirements are in direct tension; isolation
  wins here because SAB is non-negotiable for the transport.

## Validation status

The dev-server path (`cargo xtask web --serve`) is verified: all three headers
land on both the document and the `.wasm` subresource (checked with `curl -I`),
which is the precondition the spike (`SPIKE-0035-findings.md`) proved yields
`crossOriginIsolated === true` + a live SAB in a real browser.

**Real-host validation is a manual deploy step** — drop `target/web-dist/` onto
a headers-capable static host (Netlify/Cloudflare Pages read the emitted
`_headers` directly) and confirm `self.crossOriginIsolated === true` there. Not
automatable from this repo; CI deploy is out of scope (E020).
