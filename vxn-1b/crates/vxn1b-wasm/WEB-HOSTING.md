# Hosting VXN-1b on the web (ticket 0294)

**The isolation story is identical for all three synths and is written up once,
in [vxn-1's WEB-HOSTING.md](../../../vxn-1/crates/vxn-wasm/WEB-HOSTING.md).**
Read that for what COOP/COEP do, how to verify `crossOriginIsolated`, and what
each hosting provider needs. Nothing about it is VXN1b-specific, and a third
copy would only drift.

This note records what *is* specific to this port.

## Build and serve

```sh
cargo run -p vxn1b-xtask -- web            # → target/web-dist-vxn1b/
cargo run -p vxn1b-xtask -- web --serve    # …and serve it isolated on :8080
```

VXN1b has no `cargo xtask` alias (no per-product `.cargo/config.toml`, unlike
vxn-1 and vxn-2), so the package is named explicitly. The bundle directory is
`target/web-dist-vxn1b`, not the shared `target/web-dist` — the three ports get
built in the same tree and would otherwise overwrite each other.

## What ships

20 files: two wasm modules, thirteen JS modules (nine from this port's `web/`,
six shared from `crates/vxn-core-web/assets`, one worklet), the generated
`index.html`, and a `_headers`.

**No `factory.bin`.** vxn-1 and vxn-2 fetch a baked factory bank at boot; VXN1b
embeds its bank in the controller wasm (`include_dir!`, ticket 0290) and
publishes the corpus during `vxnc_new()`. There is no asset to bake, no fetch to
fail, and nothing to keep in sync with the plugin's bank — it *is* the plugin's
bank.

## Deploying

```sh
vxn-1b/crates/vxn1b-wasm/deploy-web.sh              # build, copy, commit, push
NO_PUSH=1 vxn-1b/crates/vxn1b-wasm/deploy-web.sh    # …stop before pushing
SITE=~/elsewhere ...                                # different site checkout
```

Publishes to `products/vxn-1b/web` in the `vulpus-labs-site` Hugo repo. The push
is the deploy — Netlify builds from `main`.

### The `_headers` file has one block per synth

`static/_headers` in the site repo now carries three blocks: `/products/vxn-1/web/*`,
`/products/vxn-2/web/*`, `/products/vxn-1b/web/*`. **A deploy script must append
its own block if missing, never rewrite the file.** All three do now; vxn-1's
did not until 0294, and running it would have taken the other two synths off the
air — their pages still load, then fail to construct a `SharedArrayBuffer`, which
presents as "the synth is broken" rather than "a response header went missing".

Verify after any deploy that *all three* survived:

```sh
curl -sI https://vulpuslabs.com/products/vxn-1b/web/ | grep -i cross-origin
curl -sI https://vulpuslabs.com/products/vxn-1/web/  | grep -i cross-origin
curl -sI https://vulpuslabs.com/products/vxn-2/web/  | grep -i cross-origin
```

Each must show `Cross-Origin-Opener-Policy: same-origin` **and**
`Cross-Origin-Embedder-Policy: require-corp`. One without the other is not
isolation.

## Scope: this build is a demo

Settled in [E045](../../../epics/open/E045-vxn1b-web-wasm-browser-port.md) and
[0297](../../../tickets/open/0297-vxn1b-web-demo-scope.md). Its answer to a
changed audio device or a wasm trap is *reload the page*: there is no graph
rebuild, no device-change following, no trap recovery. Persistence (IndexedDB
presets, autosave) is convenience — losing it should never be worse than
annoying.

What is **not** demo-scoped, because it is a browser fact rather than a DAW one:
the autoplay gesture gate, suspend/resume with a voice flush on the way back, and
the cross-origin isolation above. Without any of those there is no instrument.
