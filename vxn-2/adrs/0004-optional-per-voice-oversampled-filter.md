# ADR 0004 — Optional per-voice oversampled filter

- **Status:** Accepted
- **Date:** 2026-06-12
- **Scope:** Add VXN1's OTA-C ladder filter to VXN2 as an *optional*, *per-voice*
  stage sitting post-stack-sum / pre-voice-sum, with oversampling localised to
  the filter (upsample in, downsample out). Cutoff and resonance become mod
  matrix destinations.

## Context

VXN2 is a pure-FM instrument: the operator graph produces the timbre and the
signal path has no subtractive filter at all. The render loop sums each stack
to a stereo pair inside `stack_tick_stereo` (`vxn2-dsp/src/stack.rs`) and then
sums all active stacks into the dry bus in the engine's per-sample loop
(`vxn2-engine/src/engine.rs`), straight into the FX chain
(cleanup → delay → reverb → master). There is no nonlinearity anywhere in the
voice path — operators are pure sine.

We want VXN1's OTA-C ladder (`vxn-1/crates/vxn-dsp/src/ota_ladder.rs`) available
in VXN2 as a sound-design tool: a Roland/Juno-flavoured multimode resonant
filter (LP/HP/BP/Notch × 2/4-pole), self-oscillating at the top of its
resonance range. Three requirements shape the design:

1. **Optional, like the FX.** A patch that doesn't use the filter must pay
   nothing and sound bit-identical to today's output. The filter is off by
   default.
2. **Per-voice.** Each voice gets its own cutoff/resonance so the matrix can
   modulate them per-note (velocity → cutoff, mod-env → cutoff, etc.). This
   forces the filter to sit *after* a voice's signal is one stream but *before*
   voices are mixed together — i.e. post-stack-sum, pre-voice-sum.
3. **Oversampled — but only the filter.** The ladder's per-stage `tanh` is the
   only nonlinearity and the only aliasing source. We oversample *just that
   stage*: upsample each voice on the way in, run the ladder at the oversampled
   rate, downsample on the way out. The FM operators stay at base rate — unlike
   VXN1, which runs its *whole* oscillator path at the oversampled rate and
   therefore only ever needed a decimator, never an interpolator.

The control-rate state needed to render a voice (pitch smoothers, level/pan mod
ramps) was verified to be **already per-stack** (`pitch_smoothers[N_STACKS]`,
`level_mod_inc[stack][op][lane]`, all ramped into Stack-owned fields). So
reordering the render loop from sample-major to stack-major — required to render
each voice as a contiguous block for block-rate resampling — is nearly free and
needs no precompute-to-arrays step.

## Decision

### 1. Port the OTA-C ladder kernel into `vxn2-dsp`

Copy `OtaLadderKernel` / `OtaLadderCoeffs` / `FilterMode` / `FilterSlope` from
VXN1's `ota_ladder.rs` into a new `vxn2-dsp/src/filter.rs`. The scalar,
frozen-coefficient kernel (`set_coeffs` once per block, `tick(x) -> y` per
sample) is the right granularity here: the filter runs on a stack's *summed*
stereo pair, so there is no 8-lane SoA to vectorise — just two kernels per stack
(L and R). The per-sample-ramped `PolyOtaLadder` SoA sibling is **not** ported;
it solves a per-lane problem we don't have.

The kernel needs `fast_tanh`. VXN2 has none (operators are pure sine), so port
VXN1's Padé-(5,6) `fast_tanh` from `vxn-dsp/src/math.rs` into a new
`vxn2-dsp/src/math.rs`, following the established "lifted from VXN1" convention
(`smoother.rs`). `vxn2-dsp` stays dependency-free — we copy, we do not depend on
the VXN1 crate. The ±2.5 hard clamp branches are hot-path-sensitive
(per VXN1 lessons); keep the kernel's existing branch structure and re-measure
rather than refactoring the clamp.

VXN1's Moog `ladder` and standalone `hpf` are **out of scope** — the OTA ladder's
HP mode covers high-pass; the Moog variant is a separate voicing decision for
later if wanted.

### 2. Placement: per-stack, post stack-sum, pre voice-sum

A "voice" in VXN2 is a *stack* (the alloc unit; its 8 lanes are unison copies).
The filter is **per stack**, instantiated as two `OtaLadderKernel`s (L/R) living
on the `Stack` struct (or a parallel per-stack array on the engine). It consumes
the `(sl, sr)` that `stack_tick_stereo` already produces and feeds the voice-sum.

Because the filter is downstream of the lane fold, cutoff/resonance are
**per-stack scalars**, not per-lane — all unison lanes of a stack share one
cutoff. This is physically inherent to the placement, not a limitation, and is
consistent with how the signal is already collapsed.

### 3. Oversample only the filter; build the missing interpolation stage

Per voice, with oversampling factor `F ∈ {1, 2, 4, 8}`:

```
render stack block        → base-rate (sl, sr)[block]
upsample (interp halfband) → (sl, sr)[block * F]        // per voice
ladder @ F× rate, in place                               // per-voice nonlinearity
accumulate into os_bus[block * F]                        // voice-sum AT F× rate
```

The **upsampler is new work**. VXN1's `halfband.rs` ships only a *decimator*
(`Oversampler::decimate`) because VXN1 generates oscillators directly at the
oversampled rate. Here the FM stays at base rate, so we need the interpolating
counterpart: zero-stuff + the same symmetric halfband FIR (×F gain
compensation), cascaded 2× stages mirroring the decimator's A/B/C structure.
Port the decimator as-is and add `Oversampler::interpolate` (or a sibling
`HalfbandInterp`). The interpolation low-pass is **not optional** — skipping it
lets base-rate spectral images intermodulate inside the ladder.

### 4. Defer decimation to a single shared stage, post voice-sum

Summing is linear; decimation (anti-alias LP + decimate) is linear; linear ops
commute. So `decimate(Σ voices) ≡ Σ decimate(voice)` — *exactly*, not
approximately. We therefore accumulate every voice's filtered output into one
oversampled bus and decimate the **bus** once:

```
for stack in active: upsample → ladder → os_bus += voice_os
dry[block] = decimate(os_bus)        // ONE decimator, shared
fx(dry) → out                         // base rate, unchanged
```

The only per-voice cost is the upsampler + the ladder (both inherent to the
feature). The expensive decimation FIR runs **once** instead of N times. The
voice-sum moves to the F× rate (F× more adds, F×-long bus buffer) — cheap
against the FIR saving. One reusable per-voice OS scratch buffer + one OS bus
suffice; no N-buffer fan-out. FX stay at base rate, post-decimate.

### 5. Stack-major render on the filter path; sample-major bypass when off

The filter-enabled flag is checked **once per block**, selecting one of two
render bodies — no per-sample branch:

- **Filter OFF** — the current sample-major loop, byte-for-byte unchanged:
  `for sample { for stack { tick → dry += } } → fx`. This is the tuned hot path;
  it must remain the literal current code, not a degenerate case of the ON path.
  No OS buffers are allocated when off.
- **Filter ON** — stack-major:
  `for stack { render block → upsample → ladder@F → os_bus += } → decimate → fx`.

Stack-major is licensed because per-sample control state is already per-stack
(see Context); the reorder just nests the loops the other way and advances each
stack's pitch-smoother / mod-ramp inside its own inner loop.

### 6. Quiescence skip per stack

Silent ≠ quiescent: a released voice whose ladder is highly resonant keeps
ringing. The skip gate keys on **filter state magnitude**, not input level
(reusing VXN1's `silent-skip-filter-state` lesson and its high-resonance edge
case). Per stack, per block, skip the upsample + ladder when:

- the stack is idle / its amp envelope is at zero (input will be zero), **and**
- all four ladder stage states are below `eps` — `eps` chosen to cover the
  resonance ring, not merely denormals.

A skipped voice contributes exact zero to `os_bus`, so omitting its work is
exact. Skipping **freezes** ladder state and the cutoff/resonance coeff ramps
(does not clear or advance them): frozen state is already ~0, so re-entry is
glitch-free, and freezing the coeff ramps avoids a coefficient jump on
re-trigger (amp-envelope attack masks any residual discontinuity). The
quiescent flag re-arms on note-on. The shared decimator always runs regardless,
to flush its own history.

### 7. Cutoff and resonance as mod matrix destinations

Add `DestId::Cutoff` and `DestId::Resonance` to the matrix (`matrix.rs`). Both
are **per-stack scalar** destinations — per-lane matrix contributions collapse
to one value per stack (lane-0 / active-lane mean, matching the existing
per-stack aggregation pattern used for `DelayMix` / `ReverbMix`). Coefficients
are recomputed at **block rate** into `OtaLadderCoeffs` and frozen for the block,
matching the scalar kernel's contract; the per-sample coeff ramping of the
unported `PolyOtaLadder` is deliberately not adopted. If block-rate cutoff steps
prove audible as zipper noise under fast modulation, a per-block coeff ramp
(two-endpoint, like the level-mod ramp in ticket 0077) is the escape hatch — not
in v1.

### 8. Latency reporting — **reverted, VXN2 reports no latency**

The up/down halfband cascade has group delay (16 oversampled samples per stage;
`HalfbandFir::GROUP_DELAY_OVERSAMPLED`). The filter path therefore adds a fixed,
factor-dependent latency that the dry bypass path does not (≤28 base samples /
~0.6 ms at 8×; `roundtrip_latency_base_samples`).

This was originally reported to the host via the CLAP latency extension and
re-reported on enable/disable and OS-factor change. **That was reverted.** CLAP
only permits the reported latency to change across an `activate` boundary, so
every `filter-oversample` change forced a `host.request_restart()` —
deactivate/reactivate cuts all sounding voices, an audible dropout that returns
only on the next note-on. Reporting the figure correctly and switching OS
glitch-free are mutually exclusive.

VXN2 chooses glitch-free OS switching: it registers **no** latency extension and
reports `latency: 0` (matching vxn-1, which never reported its decimator delay).
The cost is ≤0.6 ms of uncompensated, OS-setting-dependent delay on the filter
path — inaudible solo, only relevant to phase-coherent layering. The latency
*truth* (and its tests) stays in `vxn2_dsp::halfband::roundtrip_latency_base_samples`
for reference; nothing consumes it. Worth reporting again only if a future host
gains seamless latency changes, or if we pad shorter factors to a constant delay
so the reported figure never moves.

### 9. Filter parameters

A new **Filter** section in the param table (`params.rs`) and `PARAMETERS.md`.
ID stability is not a constraint (per VXN1 `id-stability-dropped`), so the
section slots in after Master without churn concerns:

- `filter-enable` — bool, default **off**.
- `filter-cutoff` — Hz, log taper (matrix dest `Cutoff`).
- `filter-resonance` — `[0, 1]`, self-osc at 1 (matrix dest `Resonance`).
- `filter-mode` — enum LP / HP / BP / Notch.
- `filter-slope` — enum 2-pole / 4-pole.
- `filter-drive` — input drive into stage-0 `tanh`.
- `filter-oversample` — enum 1× / 2× / 4× / 8×.

`filter-mode`, `filter-slope`, `filter-enable`, `filter-oversample` are
topology/structural selectors (like algo / matrix source-dest) — excluded from
CLAP automation; `cutoff`, `resonance`, `drive` are CLAP-automatable and
matrix-targetable (cutoff/reso) continuous controls.

### 10. Click-free `filter-enable` toggle — equal-gain crossfade

Because the path reports no PDC (§8), the OFF and ON dry buses differ by the
resampler group delay (≤28 base samples at 8×) *and* by the saturator's
level/timbre step. Hard-switching `filter-enable` between the two render bodies
(§5) pops on both counts — the group-delay shift jumps the waveform in time, and
the saturator step lands as a discontinuity.

The toggle therefore crossfades. For one ~8 ms window (`FILTER_XFADE_MS`) the
engine renders the dry bus **both** ways from a *single* stack tick — the raw
HP'd sum (OFF dry) and the filtered signal at the configured OS factor (ON dry)
— and blends them. Rendering both from one tick is what makes the blend valid:
the two buses are the same source material differing only by the filter, so the
crossfade hides exactly the discontinuity the switch would expose. On the off→on
edge the filter + resampler state is reset so the ON bus rings up from silence
rather than a stale tail.

The curve is a **raised cosine** (`0.5 − 0.5·cos(πt)`), equal-gain (weights sum
to 1), not equal-power. Two reasons: the buses are strongly correlated (ON is
the filtered raw), so equal-gain holds amplitude without the +3 dB mid-fade bump
equal-power would add; and the raised cosine has **zero slope at both
endpoints**, so neither the engage start nor the handoff to the steady body
leaves a slope corner. An equal-power `cos` weight has slope −π/2 at t=1; when it
clamps to 0 at the handoff — right where the ON signal is full-amplitude — that
corner itself reads as a click. `t` spans the closed interval [0,1]
(denominator `len−1`) so the last fade sample lands exactly on the target.

Edge-only: the dual render and the second dry buffer run for one window per
toggle and never touch the steady-state hot paths. Verified by
`filter_toggle_is_click_free` (4th-difference click energy at both edges stays
within the tone's own slew, at 8× — worst-case group delay).

## Consequences

- The filter differs structurally from the FX "optional" idiom: FX optionality
  is a mix-blend (mix = 0 silences), whereas the filter optionality is a *render
  topology* switch (loop major-ness changes). Two render bodies must be
  maintained; the OFF body is the existing tuned loop and must not regress.
- The cost model when on: N_active × (upsample + ladder) at F× rate + one shared
  decimate. Dominant term is the ladder (serial-recursive, can't vectorise over
  time) × F. At F = 4 this is the realistic default; 8× is an escape hatch for
  extreme resonance/drive (mirrors VXN1's 8× rationale).
- Quiescence-skip is what keeps held-chord-with-released-tails affordable; its
  correctness hinges on the resonance-aware `eps`, which must be validated at the
  self-oscillation boundary.
- Host PDC does not see the filter path: VXN2 reports `latency: 0` (see §8). The
  filter adds ≤0.6 ms of uncompensated delay, traded for glitch-free OS switching
  — changing `filter-oversample` no longer restarts the plugin.
- Toggling `filter-enable` is click-free via a one-window equal-gain crossfade
  of the two dry buses (§10), the mitigation for the uncompensated group-delay +
  saturator step the no-PDC choice (§8) leaves on the edge. Edge-only cost; the
  steady OFF/ON hot paths are untouched and the OFF bypass stays bit-identical.
- `vxn2-dsp` gains `filter.rs` + `math.rs` (`fast_tanh`) + halfband
  interpolation, all ported/dependency-free. The crate's no-dependency invariant
  holds.
- The preset format gains the Filter section fields; older factory presets
  default `filter-enable = off` and are unaffected.

## Open questions

- **Per-stack coeff aggregation** — lane-0 vs active-lane mean for cutoff/reso.
  Lane-0 is cheaper and almost always indistinguishable (cutoff is rarely
  voice-spread-modulated); start there, revisit if a patch wants spread-driven
  cutoff.
- **Block-rate vs ramped coeffs** — ship block-rate frozen; promote to a
  two-endpoint per-block ramp only if zipper noise is measured under fast cutoff
  automation (§7).
- **Drive normalisation** — VXN1's OTA `drive` is pre-`tanh` gain with no
  output compensation; whether VXN2 wants make-up gain to keep enable/disable
  level-matched is a sound-design call deferred to the audibility sweep.
