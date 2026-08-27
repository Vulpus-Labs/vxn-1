# Architecture

VXN1 lives inside a Cargo workspace shared with VXN2. The product-specific crates are `vxn-dsp`, `vxn-engine`, `vxn-app`, `vxn-clap`, and `vxn-ui-web`; cross-product reusable types (Controller event enums, preset-IO, host-event plumbing) live in `vxn-core-app` at the workspace root. Layering is strictly downward — `vxn-clap` → `vxn-app` → `vxn-engine` → `vxn-dsp`. UI is a separate axis (`vxn-ui-web`).

```
              ┌───────────────┐
              │   vxn-clap    │  CLAP cdylib entry point (clack)
              └──────┬────────┘
                     │
   ┌─────────────┐   │   ┌────────────────┐
   │ vxn-ui-web  │◀──┴──▶│    vxn-app     │  Controller (MVC arbiter)
   └─────────────┘       └──────┬─────────┘
                                │
                       ┌────────▼─────────┐
                       │    vxn-engine    │  Param model, voice alloc, render loop
                       └────────┬─────────┘
                                │
                       ┌────────▼─────────┐
                       │     vxn-dsp      │  Framework-free DSP kernels
                       └──────────────────┘
```

## Crate roles

| Crate | Role |
| --- | --- |
| **`vxn-dsp`** | Framework-free, allocation-free DSP kernels: oscillators, filters, ADSR, LFO shapes, ring-mod / PM / sync, BBD chorus, FDN reverb, phaser, delay. No `std::sync`, no globals. Unit-tested with golden buffers. |
| **`vxn-engine`** | Parameter table (`SharedParams`), voice allocator, key-mode router, modulation calculation, and the block-rate render loop. Owns one rendered voice per channel and the global FX rack. Calls into `vxn-dsp` per sample. |
| **`vxn-app`** | The VXN1 Controller. Holds non-automatable state (key mode, split point), defines VXN1-specific custom events (`Vxn1UiCustom`, `Vxn1ViewCustom`), and re-exports the shared `UiEvent` / `HostEvent` / `ViewEvent` enums from `vxn-core-app`. |
| **`vxn-core-app`** | Workspace-shared controller substrate: generic `UiEvent` / `HostEvent` / `ViewEvent` types, preset-IO scaffolding, MIDI/automation plumbing. Reused by VXN2. |
| **`vxn-ui-web`** | The View. wry-WebView host for the HTML/CSS/JS faceplate (E010/E011). Pluggable — the controller doesn't depend on a specific view implementation. |
| **`vxn-clap`** | The CLAP shell. `clack` cdylib that wires the audio thread to `vxn-engine`, the main thread to `vxn-app`, and the GUI extension to `vxn-ui-web`. |
| **`xtask`** | Build/bundle helper. Drives `cargo build`, packages the CLAP bundle, optionally invokes clap-wrapper CMake for VST3. |

## Threading model

Three contexts, with strict rules about who writes what:

| Thread | Reads | Writes |
| --- | --- | --- |
| **Audio** (RT) | `SharedParams` atomics | Voice state (private); audio buffer |
| **Main** | Anything | `SharedParams` (via Controller events); plugin state |
| **GUI** | Posted `ViewEvent`s | UiEvents → Controller |

The audio thread never blocks on the main thread. The main thread never directly mutates voice state — it goes through `SharedParams` atomics and lets the audio thread pick up the change on the next control block boundary.

See [MVC layering](mvc.md) for the event-channel topology.

## Build configuration

VXN1 builds with stable Rust 1.85+ (edition 2024). Key build-time settings:

- **Optimisation**: `release` profile with `lto = "thin"` and `codegen-units = 1` for the cdylib crates.
- **Target features**: `+neon` on aarch64-apple-darwin; `+avx2` on x86_64 targets.
- **No-std subset**: `vxn-dsp` is `#![no_std]`-compatible but enables `std` by default for `f32` math helpers.

## Workspace layout

```
vxn-1/vxn-1/
├── crates/
│   ├── vxn-app/        Controller
│   ├── vxn-clap/       CLAP shell
│   ├── vxn-dsp/        DSP kernels
│   ├── vxn-engine/     Param model + render loop
│   └── vxn-ui-web/     WebView GUI
├── xtask/              Bundler / build tooling
├── adrs/               Architecture decision records
├── epics/              Multi-ticket project epics
├── tickets/            Open and closed tickets
├── tests/              Integration tests
└── docs/               This manual
```

## Future-proofing

The architecture is designed to allow a second instrument (VXN2) to reuse the Controller, shell integration, WebView embedding, and preset format. VXN2 defines its own Synth + parameter blocks (DX7-style 6-op FM); everything around the synth core is shared infrastructure (see ADR 0007 and the VXN2 ADRs).
