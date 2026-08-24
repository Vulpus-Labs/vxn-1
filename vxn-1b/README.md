# VXN1b

Dual-layer subtractive polysynth, CLAP + VST3. VXN1's sound engine with its
fixed modulation panels replaced by a generic 16-slot mod matrix.

VXN1b is a **sibling product, not a version of VXN1**. VXN1 continues to ship
its fixed-panel surface; the two install side by side under separate plugin ids
(`labs.vulpus.vxn1` and `labs.vulpus.vxn1b`) and neither reads the other's
presets.

## What differs from VXN1

The DSP does not. VXN1b takes a direct crate dependency on
[`vxn-1/crates/vxn-dsp`](../vxn-1/crates/vxn-dsp) rather than copy-adapting it,
so the oscillators, ladder, envelopes and FX kernels are the same code — the
aim is zero divergence in how it sounds. Three things diverge:

- **Routing is a matrix.** VXN1's Pitch Mod / PWM Mod / Filter Mod / Mod Wheel /
  Pitch Wheel panels are gone. In their place, 16 slots per layer, each a
  `source → destination` pair with a depth, a curve (linear, exponential,
  logarithmic, bipolar) and a secondary scale source that acts as a per-route
  VCA. Sources: Env 1/2, LFO 1/2, velocity, key, mod wheel, pitch wheel,
  aftertouch, per-note random, stack spread. Destinations: pitch, cross-mod
  sweep, cross-mod amount, PWM (both oscillators together or each alone),
  cutoff, resonance, HPF cutoff, amp, pan, Env 1/2 time scale, Env 1/2 sustain,
  and LFO 1 rate.

  The trade is deliberate and stated in [ADR 0001](adrs/0001-vxn1b-overall-design.md):
  more flexible routing and a more compact panel, at the cost of more opaque
  patch programming. VXN1's ADR 0004 made the opposite call for that instrument.

- **Two layers.** A patch is two independent parameter sets (Layer 1 / Layer 2)
  with their own matrices, playing together or split — see
  [ADR 0002](adrs/0002-vxn1b-dual-layer.md). Level, pan, detune and mute are
  per layer; tuning, volume, drift, the limiter, oversampling and the FX chain
  are global.

- **Stack width and voice mode are orthogonal.** VXN1's single AssignMode splits
  into lanes-per-note (1–32) and Poly/Solo, with legato as its own toggle
  ([ADR 0003](adrs/0003-vxn1b-stack-width-and-voice-mode.md)).

Everything else carries over: 2 oscillators + sub + noise, hard-sync / PM / ring
cross-modulation, a 4-pole ZDF ladder with a switchable HPF, two ADSRs, two
LFOs, and the chorus / phaser / delay / reverb / dynamics chain.

## Parameters

The host sees **181 parameters**: 73 per layer × 2, plus 35 globals. The layer
parameters are a flat two-layer map over an otherwise unchanged per-synth table
— a Layer 1 control and its Layer 2 twin are separate automation targets.

Matrix *topology* (which source feeds which destination, through which curve) is
not automatable; it lives in the patch state and is edited in the faceplate's
matrix overlay. Only the 16 depths per layer are host parameters, which is what
lets a slot be automated without the routing changing underneath it.

## Design docs

- [ADR 0001 — overall design](adrs/0001-vxn1b-overall-design.md)
- [ADR 0002 — dual layer](adrs/0002-vxn1b-dual-layer.md)
- [ADR 0003 — stack width and voice mode](adrs/0003-vxn1b-stack-width-and-voice-mode.md)
- [PARAMETERS.md](PARAMETERS.md) — the full table, generated from the engine.

Tickets live in the repo-root `tickets/` counter shared across the vxn products
(VXN1b work is tagged `product: vxn-1b`).

## Building

macOS and Windows. From the repo root:

```sh
./vxn-1b/deploy.sh                # build + install the CLAP
./vxn-1b/deploy.sh --vst3         # also build + install the VST3
./vxn-1b/deploy.sh --universal    # macOS: arm64 + x86_64 in one binary
./vxn-1b/deploy.sh --bundle-only  # stage in target/bundled/, don't install
./vxn-1b/deploy.sh --uninstall    # remove installed artifacts
```

Unlike vxn-1 and vxn-2, VXN1b has no `cargo xtask` alias (no per-product
`.cargo/config.toml`), so `deploy.sh` calls the xtask package directly:
`cargo run -p vxn1b-xtask -- <subcommand>`. The build is always release.

The VST3 path wraps the same staticlib through
[clap-wrapper](https://github.com/free-audio/clap-wrapper) and needs CMake plus
the repo-root submodules:

```sh
git submodule update --init --recursive
```

Install destinations (macOS):

- `~/Library/Audio/Plug-Ins/CLAP/vxn1b.clap`
- `~/Library/Audio/Plug-Ins/VST3/VXN1b.vst3`

## Releases

VXN1b versions independently of the shared `0.x` line vxn-1 and vxn-2 ride, and
tags `vxn-1b-<version>` (the first is `vxn-1b-0.0.1`). Publishing such a tag as
a GitHub Release builds and attaches the macOS universal and Windows x64
CLAP/VST3 artifacts; the workflow's per-job tag guards keep the vxn-1 and vxn-2
jobs out of a VXN1b release and vice versa. See
[`.github/workflows/release.yml`](../.github/workflows/release.yml).

## Crates

| Crate | Role |
|---|---|
| [`vxn1b-engine`](crates/vxn1b-engine) | Parameter table, matrix evaluator, block render loop, preset store |
| [`vxn1b-clap`](crates/vxn1b-clap) | CLAP shell (clack) — params, state, GUI and timer extensions |
| [`vxn1b-ui-web`](crates/vxn1b-ui-web) | The HTML faceplate and matrix overlay, spliced into a wry WebView |
| [`vxn1b-xtask`](xtask) | Bundling: `.clap` / `.vst3`, universal builds, install |

The DSP itself is [`vxn-dsp`](../vxn-1/crates/vxn-dsp), shared with VXN1;
`vxn-core-*` and `vxn-preset` at the repo root are shared with every product.
