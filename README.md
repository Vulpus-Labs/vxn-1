# VXN

Monorepo for [Vulpus Labs](https://github.com/Vulpus-Labs) synthesizers.

| Subdir | Status | Notes |
| --- | --- | --- |
| `vxn-1b/` | shipping | **Canonical virtual-analogue synth.** Two-layer subtractive polysynth with a generic mod matrix, HTML faceplate, CLAP + VST3, and a browser build. See [vxn-1b/README.md](vxn-1b/README.md). |
| `vxn-2/` | shipping | DX7-lineage 6-operator FM with first-class voice stacking, per-voice oversampled filter, browser build. |
| `vxn-3/` | in flight | Sample-free synthesis drum machine, pattern-first. |
| `archive/vxn-1/` | **retired** | The original 80s-style analogue polysynth. Retired 2026-08-27 — see below. |
| `crates/` | — | Shared `vxn-core-*` layers plus `vxn-core-dsp` and the `vxn-asm-check` codegen guard. |

One flat Cargo workspace, one `Cargo.lock`, one `cargo test --workspace`. Each
synth keeps its own `xtask` for bundling.

## Releasing

vxn-1b and vxn-2 ship together: one version, one bare-semver tag, one release
page, eight assets. `cargo release all <version>` drives the whole process —
verify, bump, tag, wait for the builds, deploy both browser bundles, repoint the
[product pages](https://vulpuslabs.com/products/) — confirming before each
irreversible step. See **[RELEASING.md](RELEASING.md)** for the process, the
manual equivalent of every step, and what to do when one fails mid-release.

## vxn-1 retirement (2026-08-27)

vxn-1 shipped first and defined a lot of what the others inherited — the MVC
split, the poll-and-diff pump, the analogue DSP kernels. **vxn-1b is now the
canonical virtual-analogue synth**: it began as a matrix-modulation variant of
vxn-1 and has since overtaken it.

`archive/vxn-1/` is **not a workspace member and is not expected to compile.**
It is kept for reference, not for building; shared crates evolve without regard
for it. Do not add it back to `[workspace] members`.

What survived the move:

- **`vxn-dsp`** — vxn-1's DSP kernel set is vxn-1b's, and always was (it reused
  it verbatim). It now lives at [`vxn-1b/crates/vxn-dsp`](vxn-1b/crates/vxn-dsp),
  name unchanged so call sites and the shared crates' re-exports did not churn.
- Everything vxn-1 contributed to `crates/vxn-core-*` before retirement — the
  Controller, `ParamModel`, the preset system, the web transport.

What went with it:

- vxn-1's engine, CLAP shell, web port, faceplate, manual and `xtask`.
- The **vxn-1b↔vxn-1 parity oracle** (`vxn1b-engine/tests/parity.rs`,
  `taper_parity.rs`). It asserted vxn-1b renders identically to vxn-1 for
  equivalent patches; with no reference left, that question is retired too.
  vxn-1b keeps its own suites, `zipper_regression`, the declick tests and its
  goldens — but the cross-synth check is gone, and that is a real reduction in
  cover worth knowing about.
- `BypassXfade` and `fade_len_samples` from `vxn-core-utils`, which had no
  consumers left once vxn-1 went (see [ADR 0002](adrs/0002-vxn-core-dsp.md) §5).

License: see [LICENSE.txt](LICENSE.txt) (MIT OR Apache-2.0).
