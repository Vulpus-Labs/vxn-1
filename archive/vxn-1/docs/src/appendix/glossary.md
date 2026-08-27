# Glossary

**ADSR** — Attack / Decay / Sustain / Release. The four envelope stages.

**Assign Mode** — Per-layer policy for how voice channels are spent: Poly, Unison, Solo, Twin.

**BBD (Bucket-Brigade Device)** — Analogue delay line implemented as a charge-coupled shift register. VXN1's chorus models a Juno-60-style BBD with bucket saturation and a reconstruction filter.

**CLAP** — [CLever Audio Plug-in API](https://cleveraudio.org/). The native plugin format VXN1 uses. MIT-licensed, modern, no vendor lock-in.

**Channel** — One slot in a layer's 8-slot voice pool. Each layer has 8 channels; a voice (one "note's worth of synthesis state") occupies one or more channels depending on Assign Mode.

**Control block / Control rate** — 32-sample chunk inside which modulation, envelope, LFO, and filter coefficients are constant. ~1.5 kHz update rate at 48 kHz sample rate.

**Cross-Mod** — Umbrella term for the four oscillator-interaction modes: Off, Sync, FM (PM), Ring.

**Detune** — Tuning offset, typically in cents (ct).

**Dual mode** — Key mode in which both layers receive every note and play different patches simultaneously (8 + 8 layered stereo).

**Env 1 / Env 2** — The modulation and amplitude envelopes, respectively. Env 2 is hardwired to the VCA; Env 1 is freely routable.

**FDN (Feedback Delay Network)** — Reverb topology built from a matrix of delay lines with feedback. VXN1's reverb is an 8-channel FDN.

**FM** (in VXN1 UI) — Labelled "FM" on the cross-mod selector, but internally implemented as **PM** (phase modulation). Same family of timbres, but pitch stays stable when the modulator's DC level moves.

**Free-Run** — LFO 1 setting that keeps the LFO phase continuous across note-ons (vs. retriggering on each note).

**Glide / Portamento** — Per-voice pitch slide between notes.

**HPF (High-pass filter)** — 1-pole pre-VCF filter on each voice's mix.

**Key Mode** — Instrument-level routing: Whole / Dual / Split. Decides what notes go to which layer. Not automatable.

**Key Track** — Filter feature that ties cutoff to the played note (in VXN1: binary on/off, 100% per octave when on).

**Layer** — One of two complete patches (Upper / Lower). Each layer has 8 channels. Always-allocated regardless of Key Mode.

**LFO 1 / LFO 2** — Low-frequency oscillators. LFO 1 is per-voice; LFO 2 is global.

**Mod-wheel** — MIDI CC1. VXN1 has four fixed Mod-wheel destinations: PWM, Cutoff, Reso, Cross-Mod Sweep.

**OTA-C** — Operational Transconductance Amplifier with Capacitor. The analog topology behind VXN1's ladder filter (R3109 / IR3109 family).

**Oversample** — Synthesis runs at 1× / 2× / 4× / 8× the host sample rate to reduce aliasing. Per-voice path is oversampled; effects are not.

**Patch** — A preset that holds one layer's state. Loads into Upper or Lower.

**Performance** — A preset that holds the full instrument state (both layers + global + Key Mode + Split Point).

**PM (Phase Modulation)** — One of the cross-mod modes; labelled "FM" on the UI.

**polyBLEP** — Band-limited step residual. Algorithm that anti-aliases saw/pulse/sync edges by subtracting a windowed step from the naive waveform.

**Poly mode** — Standard polyphonic Assign Mode: first-free voice, oldest-steal.

**PW (Pulse Width)** — Static duty cycle of the pulse waveform. Modulated by the PWM route.

**Ring Modulation** — Multiplication of two signals. VXN1's ring uses a Parker diode-bridge model.

**SharedParams** — The atomic parameter table. The audio thread reads it directly; the main thread mediates writes via the Controller.

**Solo mode** — Monophonic Assign Mode with last-note priority. Legato controls envelope retrigger on slurs.

**SoA (Structure of Arrays)** — Memory layout where each field of an "object" is a separate array. Enables vectorisation across objects.

**Split mode** — Key mode in which the layers split at a MIDI note: notes below → Lower, at-or-above → Upper.

**Split Point** — MIDI note number at which Split mode partitions the keyboard. Default 60 (C4). Not automatable.

**Sub-oscillator** — Square wave one octave below Osc 1. Has its own mixer level.

**Sync** — Hard sync. Osc 1's phase is reset by Osc 2's phase wrap (Osc 2 is the master). Band-limited via polyBLEP.

**Twin mode** — Assign Mode with two channels per note, ±Detune apart. Halves polyphony to thicken each note.

**Unison mode** — Assign Mode with all 8 channels stacked per note. Mono. Per-channel detune.

**VCA (Voltage-Controlled Amplifier)** — The amp stage. Always driven by Env 2 (unless Amp Gate bypasses).

**VCF (Voltage-Controlled Filter)** — The main filter. VXN1's is a 4-pole OTA-C ladder.

**Velocity** — MIDI note-on velocity. Routes via the Vel→Cutoff knob on the filter modulation panel.

**VST3** — Steinberg's plugin format. VXN1's VST3 binary wraps the CLAP cdylib via [clap-wrapper](https://github.com/free-audio/clap-wrapper).

**Whole mode** — Key mode in which both layers play the same patch (16-voice mono-timbral).

**xtask** — Cargo workspace helper crate that drives non-cargo build steps (CLAP bundling, VST3 wrapper CMake invocation, code signing).
