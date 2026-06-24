# VXN1

**VXN1** ("vixen 1") is an 80s-style analogue polysynth by [Vulpus Labs](https://github.com/Vulpus-Labs),
built in Rust as a [CLAP](https://cleveraudio.org/) plugin.

16-voice polyphony, dual oscillators with cross-modulation, a 4-pole ladder
filter, and a vintage-flavoured chorus — packaged as a single `.clap` bundle.

## Features

- **16-voice polyphony** with per-voice envelopes and a global LFO.
- **Dual oscillators** with hard-sync and phase/cross-modulation, plus optional
  anti-aliasing oversampling up to 8×.
- **OTA-C ladder VCF** (R3109/IR3109-style) with a switchable
  12 / 24 dB/oct slope, plus a separate high-pass filter.
- **Pitch envelope** alongside the amplitude and filter envelopes.
- **Vintage bucket-brigade (BBD) chorus** — "Bright" voicing with
  bucket saturation, anti-image/reconstruction filter banks, and inverted-LFO mono-compatible stereo.
- **HTML faceplate** GUI (wry WebView) embedded via CLAP's `gui` extension.

## Architecture

VXN1 is a Cargo workspace:

| Crate           | Role                                                                       |
| --------------- | -------------------------------------------------------------------------- |
| `vxn-dsp`       | Framework-free, allocation-free DSP kernels (oscillators, filters, ADSR…). |
| `vxn-engine`    | Parameter model, voice allocation, and block-rate render loop.             |
| `vxn-app`       | Controller (MVC arbiter — ADR 0007); model trait, event types.             |
| `vxn-ui-web`    | wry-WebView plugin GUI (HTML faceplate; E010/E011).                        |
| `vxn-clap`      | [clack](https://github.com/prokopyl/clack) cdylib — the CLAP entry point.  |
| `xtask`         | Bundler / build tooling.                                                   |

**Processing model:** DSP kernels run per-sample (the recurrences are serial and
kept bit-faithful to their [`patches`](https://github.com/Vulpus-Labs) origins).
The engine drives fixed 32-sample control blocks (`CONTROL_BLOCK`), recomputing
modulation and filter coefficients once per block while the inner per-sample
loop stays branch-light.

## Building

```sh
cargo build --release
cargo xtask bundle --release            # produce the VXN1.clap bundle
cargo xtask bundle --release --install  # …and install it locally
```

Requires Rust 1.85+ (edition 2024).

### Submodules (VST3 only)

The VST3 build path (`cargo xtask bundle --format vst3`, E010) links
[clap-wrapper](https://github.com/free-audio/clap-wrapper) against the
[VST3 SDK](https://github.com/steinbergmedia/vst3sdk), both vendored as git
submodules under `vendor/` at the repo root. Initialise them before building
VST3:

```sh
git submodule update --init --recursive
```

The CLAP path (`cargo xtask bundle [--format clap]`, the default) does **not**
need the submodules — a fresh clone can build CLAP with no extra setup.

## License

Licensed under the [MIT License](LICENSE.txt).
