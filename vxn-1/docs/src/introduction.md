# Introduction

**VXN1** ("vixen 1") is an 80s-style analogue polysynth by [Vulpus Labs](https://github.com/Vulpus-Labs), built in Rust as a [CLAP](https://cleveraudio.org/) plugin. A VST3 build via [`clap-wrapper`](https://github.com/free-audio/clap-wrapper) is on the roadmap (see [Distribution](internals/distribution.md)).

This manual covers installation, the faceplate panel-by-panel, performance features (key modes, presets, MIDI), and engine internals. Read top-to-bottom for an end-to-end tour, or jump to the [Parameter reference](parameter-reference.md) and [Glossary](appendix/glossary.md) for lookups.

## What VXN1 is

A two-layer subtractive polysynth with **16 voices total** (8 channels per layer). Each voice runs:

- **Two oscillators** (Sine / Triangle / Saw / Pulse), plus a square sub-oscillator below osc 1 and a White/Pink noise source.
- **Cross-modulation** between the oscillators: band-limited hard **Sync**, through-zero **Phase Modulation** ("FM" in UI labels), or diode-bridge **Ring** modulation.
- **OTA-C ladder filter** (R3109/IR3109-flavoured) with LP / HP / BP / Notch modes and a 12 / 24 dB/oct slope switch. Separate pre-VCF high-pass filter.
- **Two ADSR envelopes** (modulation + amplitude) with linear or exponential shapes.
- **Per-voice LFO 1** (retriggered or free-running) plus a **global LFO 2** shared across both layers.
- **Fixed-route modulation panels**: pitch, PWM, filter cutoff, cross-mod sweep, mod-wheel. No matrix — every route is labelled.

The instrument-level signal path adds a **pre-chorus phaser**, a **vintage BBD chorus** (Juno-60-flavoured), a **stereo delay**, and an **FDN reverb**, finished with a master volume, optional brick-wall limiter, and selectable **oversampling** at 1× / 2× / 4× / 8×.

## What VXN1 is not

- Not a matrix-modulation synth — modulation routes are fixed and named (ADR 0004).
- Not a wavetable or sample-based instrument — strictly analogue-modelled subtractive.
- Not a multitimbral workstation — two layers, one patch role each (Upper / Lower), no per-key splits beyond a single split point.
- Not configurable for runtime polyphony — voice counts are static (8 per layer, 16 total).

## Layout of this manual

| Section | What it covers |
| --- | --- |
| [Getting started](install.md) | Install the plugin, run it for the first time, build a mental model. |
| [Faceplate reference](panels/overview.md) | One page per panel on the faceplate, in signal-flow order. |
| [Performance](key-modes.md) | Key modes, presets, MIDI, and the full parameter table. |
| [Internals](internals/architecture.md) | Engine architecture, MVC layering, distribution. For developers and curious users. |
| [Appendices](appendix/glossary.md) | Glossary and changelog. |

Parameter names and ranges in this manual come directly from `vxn-engine/src/params.rs` and the ADRs in `vxn-1/adrs/`. Where an ADR carries decision context that didn't make the manual, cross-references are noted inline.
