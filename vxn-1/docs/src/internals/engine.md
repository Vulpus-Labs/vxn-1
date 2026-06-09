# Engine & DSP

The engine is the bridge between the parameter table and the audio buffer. Its job, per host buffer:

1. Receive parameter snapshots from `SharedParams`.
2. Receive note events from the host (translated via the key-mode router; 165 params total in `SharedParams`).
3. Allocate / steal voice channels per assign mode.
4. For each control block (32 samples), recompute modulation, envelope, LFO, filter coefficients.
5. For each sample in the block, run per-voice DSP and mix into the output.
6. Apply global FX, master volume, optional limiter.

## Constants

| Constant | Value | Notes |
| --- | --- | --- |
| `CONTROL_BLOCK` | **32 samples** | Control-rate update cadence. At 48 kHz ≈ 0.67 ms. |
| Voices per layer | 8 | Static. Not reconfigurable. |
| Layers | 2 (Upper, Lower) | Always allocated. |
| Total voices | 16 | All 16 channels active in Whole mode (round-robin). |
| `MAX_OVERSAMPLE` | 8 | Per-voice buffer sized to 8× host buffer. |
| Default oversample | 2× | Real-time switchable. |
| Mod-wheel smoothing | 40 ms | Control-rate one-pole on CC1. |

## Block structure

Each call to the host's process callback:

```
loop {
    let frames_this_pass = min(host_frames_left, CONTROL_BLOCK);

    update_control_rate(snapshot, frames_this_pass);   // mod, env, LFO, filter coeffs
    for sample in 0..frames_this_pass {
        for voice in active_voices { voice.render_sample(); }
        mix_voices_into_block(buffer, sample);
    }
    apply_fx_block(buffer, frames_this_pass);

    host_frames_left -= frames_this_pass;
    if host_frames_left == 0 { break }
}
```

Modulation, envelope segments, LFO output, and filter cutoff are recomputed *once per block*. Per-sample work is the oscillator phase increment, sub/noise output, mix, filter recurrence, and VCA — all of which need per-sample precision to preserve the DSP recurrences' transient response.

## SoA voice layout

Each layer holds its 8 channels as a **structure-of-arrays** layout. Oscillator phase, mix levels, filter state, and envelope state live in 8-wide `f32` arrays. The inner loop iterates over channels, which lets the compiler auto-vectorise to NEON on Apple Silicon (and AVX2 on Intel).

A few hot-path subtleties:

- **Runtime enum matches inside the SoA loop defeat vectorisation**. Waveform selection and ladder-mix selection are hoisted out via type markers (`WaveKind`, `LadderMix`) — the loop sees a monomorphised constant, not a runtime `match` (see memory entry "VXN1 SoA match defeats SIMD" for the gotcha).
- **Silent voices skip the per-sample loop entirely** at block granularity. The cost of an idle voice is one branch per block — measured at ~1100× real-time on M1.
- **Filter state is frozen** during silent-skip. Coefficient ramps freeze too. This shows on attack of a high-resonance patch coming out of silence; the amp envelope masks staleness in practice.

## DSP kernels

`vxn-dsp` is the kernel library. All kernels are framework-free (no global state, no `std::sync`, no allocator after construction) and take an `&mut self` + per-sample inputs.

| Kernel | Notes |
| --- | --- |
| **Oscillators** | polyBLEP-band-limited saw / pulse. Sine and triangle are unaliased. PM / Sync / Ring routed through dedicated kernel variants for the SoA-friendly fast path. |
| **Sub** | Square wave at Osc 1's frequency / 2. Band-limited. |
| **Noise** | White: `Xorshift32`. Pink: 4-octave Voss-McCartney summing. |
| **Ladder filter** | OTA-C transistor-ladder. `tanh` saturator at each integrator input (rational Padé(5,6) approximation from `vxn-dsp::math`), not in the feedback path. Per-block coefficient recompute, per-sample state advance. Mode (LP/HP/BP/Notch) is a const-selected tap. |
| **HPF** | 1-pole (6 dB/oct) topology-preserving high-pass. |
| **Envelope** | ADSR with linear or exponential segments. Branch-free per-sample step; segment transitions are one branch per gate event. |
| **LFO** | Six shapes (sine, tri, saw+, saw−, sq, S&H). Phase accumulator with optional host-tempo sync. |
| **Phaser** | 4-stage all-pass with LFO-modulated centre. |
| **Chorus** | BBD model with bucket saturation, reconstruction filter, inverted-LFO stereo. |
| **Delay** | Stereo delay line, one-pole high-frequency damping on the feedback path. |
| **Reverb** | FDN, 8-channel, with damping on each loop. |

## Sample-rate handling

Sample rate is set once at plugin activation (CLAP `clap_plugin.activate`). On activation, the engine:

1. Recomputes envelope time-to-coefficient constants.
2. Recomputes LFO phase increments for all rates.
3. Recomputes filter pre-warp constants.
4. Reallocates the oversampled buffer for the per-voice render path.

Mid-stream sample-rate change is not supported (per CLAP spec, the host must deactivate / reactivate the plugin).

## Performance ballpark

Recent measured numbers on Apple M1 (release LTO build):

- **Idle (no notes)**: ~1100× real-time.
- **Single-voice dry sound (no FX)**: ~51× real-time.
- **Single-voice sync sound (cross-mod active)**: ~41× real-time.
- **16-voice full poly + chorus + delay + reverb at 4× oversample**: comfortably real-time on M1; check your own CPU budget for heavy ensembles.

See `vxn-1/crates/vxn-engine/benches/` for the `busy_profile` benchmark harness.
