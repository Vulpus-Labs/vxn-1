---
id: "0012"
title: Master out + parameter table assembly
priority: medium
created: 2026-06-05
epic: E001
---

## Summary

Final stage of the kernel: master tune + master volume + the assembled
CLAP-facing parameter table that every prior ticket has been writing into.
This is the surface the CLAP shell (later epic) and the production UI bind
against.

No FX is added here — delay (0010) and reverb (0011) sit between the voice
sum and master out.

## Acceptance criteria

- [x] `master_tune` adds cents to every voice's pitch (applied at the voice
      level, summed with bend + glide).
      *Mirrored from patch-level `master-tune` into both
      `Patch.upper.voice.master_tune_cents` and the Lower equivalent at
      snapshot time; DSP path bakes it into per-op base phase increments at
      note-on. Changes mid-note re-apply on the next note-on.*
- [x] `master_volume` is the final output gain. Range −60..+6 dB, exponential
      taper.
      *Stored as plain dB with `Taper::Linear`; the dB → linear conversion
      happens once per block in `MasterState::refresh`. A linear-in-dB fader
      IS exponential in amplitude (-60 dB = 0.001, +6 dB = 2.0), so the
      taper kind doesn't need to be `Exp` to get the "exponential" feel.*
- [x] Output limiter explicitly NOT implemented (per ADR §10 review and the
      Master panel decision).
- [x] `vxn2-engine::params::PARAMS` enumerates every CLAP-automatable
      parameter from `PARAMETERS.md`. Total = 343
      (`162 per-layer × 2 + 19 patch-level`).
      *The ticket originally counted 174 for a single layer + patch-level;
      with the per-layer doubling that PARAMETERS.md introduced for
      Layer / Split mode, the actual table is 343. The per-section counts
      in the original ticket all still match:*
      - 126 per-op (6 × 21) per layer
      - 1 algo, 4 LFO1 (patch), 5 LFO2 (per layer), 9 Pitch EG (per layer),
        5 Mod Env (per layer), 3 Assignment (per layer), 5 Stacking (per
        layer), 6 Delay (patch), 5 Reverb (patch), 2 Voicing (patch), 2
        Master (patch), 8 Mtx-slot-depth (per layer)
      *Per-op enumeration excludes `ratio_mode`, `ks_l_curve`, `ks_r_curve`
      (discrete topology selectors; preset state only — not continuously
      automatable). That's the 21-per-op count.*
- [x] Each `ParamDesc` carries: id (stable string, kebab-case),
      display name, plain range, normalised range (always [0, 1]), default,
      unit string, plain↔normalised converters.
      *`ParamKind` discriminates Float / Int / Bool / Enum and carries
      taper + unit; `to_normalised` / `from_normalised` are taper-aware.*
- [x] Parameter writes thread-safely apply to the engine state.
      *`SharedParams` is a flat `[AtomicU32; 343]` of plain f32 bits with
      `Relaxed` ordering — same shape as VXN1's `vxn-engine::SharedParams`.
      The audio thread snapshots into `EngineParams` once per control block.*
- [x] Mod matrix slot **topology** (`source`, `dest`, `curve` + slots 9–16
      `depth`) is NOT in `ParamTable` — patch state, serialised separately.
      *Updated from the original Notes: slots 1–8 `depth` per layer ARE
      CLAP-automatable per PARAMETERS.md / [`matrix.rs`]
      (`N_CLAP_DEPTH_SLOTS = 8`). The 16 CLAP depth params are in the table;
      everything else about the matrix stays patch state.*
- [x] Integration test: a stub host (in-process) sends note-on, sets each
      parameter through its full range, renders audio per setting, asserts
      no NaN / Inf.
      *`tests/param_sweep.rs`. `every_param_sweep_keeps_audio_finite_fast`
      runs in CI (3 points × 8 blocks per setting); the spec's "1 second
      per setting" variant lives behind `#[ignore]` as
      `every_param_sweep_keeps_audio_finite_full_second` (~30 min full
      run). Plus dedicated tests for silence-at-min-volume,
      audibility-at-max-volume, FX bypass round-trip, and
      every-one-of-32-algos sustained-render.*
- [x] Bench: `master_chain_full` (8 notes × Layer-mode → 16 stacks × density
      4 × full FX chain).
      *Plus `master_chain_fx_off` to isolate FX overhead. On the
      development host:*
      - `master_chain_full`: ~239 µs / 256 samples → ~22× realtime
      - `master_chain_fx_off`: ~228 µs / 256 samples → ~23× realtime
      *FX overhead is ~5% of the voice loop at this density.*

## Notes

Stable parameter IDs are no longer load-bearing for VXN1's preset format
(per memory: `vxn1-id-stability-dropped`) — VXN2 inherits this. Use kebab-
case string IDs (`op1-ratio`, `lfo2-delay`) but treat them as freely
renameable; preset format is name-keyed.

The 343-param table is built via three macros (`op_block_arr!`,
`per_layer_rest_arr!`, plus a flat patch-level array) and two const-fn
flatteners (`concat_per_layer`, `concat_all`) so the final
`const PARAMS: [ParamDesc; 343]` lives in `.rodata` without a build script.

Saturation: even without a limiter, output should not clip when individual
voices are reasonable but their sum exceeds 1.0. The master volume default
of −6 dB gives 6 dB of headroom for typical patches; patches that exceed
this are the user's responsibility (their DAW limits it).

Bench numbers from this ticket establish the kernel's headline CPU budget.
At 256-sample blocks on the dev host, 16 stacks × density 4 + delay + FDN
reverb + master runs at ~22× realtime — comfortably under the
1-block-per-block budget. See `master_chain_full` results above.

## Per-layer assignment caveat

The param table exposes `upper-assign-mode` / `lower-assign-mode` (and the
companion legato + glide-time) per layer for forward compatibility — the UI
expects a Split-mode bass-mono / lead-poly setup to round-trip via state
serialisation. The current `PolyAlloc` takes a single `AllocParams` per
note-on; v1 reads Upper's assignment block into the engine's live
`AllocParams`. Lower's entries remain in the store (visible to host
automation) but inert until the allocator is refactored to take a per-
layer assignment.

This is a known v1 limitation, not a bug. Documented in `shared.rs` module
doc + tested by `snapshot_uses_upper_assignment`.

## Matrix wiring

The matrix engine exists (ticket 0008) and is exercised by its own tests +
bench. This ticket wires CLAP-automatable slot depths (1..=8 per layer)
into the engine's `PatchMatrix` each control block, but does **not** thread
the matrix's per-lane destination accumulator (`LaneDestVals`) into the
per-sample DSP path. That integration belongs to a follow-up ticket (the
matrix has the source / dest plumbing and a per-block accumulator; the DSP
hooks for applying destinations to op-level params are not yet wired).

For ticket 0012 acceptance — render audio, sweep params, ship master out —
this gap is invisible to the sweep test (the matrix accumulator is computed
but unread, so finite-output guarantees hold). The follow-up will plumb the
matrix outputs into op-ratio / op-level / op-detune / etc. modulation
inputs.
