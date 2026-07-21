---
id: "0191"
product: vxn-1
title: "Oversampling-change declick — raised-cosine fade-in after decimator reset"
priority: medium
created: 2026-07-07
epic: E035
depends: ["0190"]
---

## Summary

Changing the output oversampling factor clicks. `on_os_change` hard-resets both
rate-specific FIR decimators when the factor moves
([lib.rs:356-362](../../vxn-1/crates/vxn-engine/src/lib.rs#L356-L362)); the FIR
then settles from zero state, and that settle lands as an audible transient. The
pre-switch audio is continuous — only the post-reset settle is the problem — so a
short **fade-in on the decimated output** buries it. No dual-rate crossfade
(would need voices rendered at two OS rates; too heavy for a rare user action).

Latency stays unreported: vxn-1 declares no CLAP `PluginLatency` extension, so an
OS change does not force a host restart (matches vxn-2's reverted 0086). Keep it.

## Design

**[lib.rs `OutputStage`](../../vxn-1/crates/vxn-engine/src/lib.rs#L315)**

Add a fade-in counter to `OutputStage`:

```rust
os_fade_remaining: usize,  // 0 ⇒ steady; > 0 ⇒ fade-in in flight
os_fade_len: usize,        // ~5 ms @ base rate
```

- In `on_os_change`, when the factor actually changes (the existing `os !=
  self.last_os` branch that already resets the decimators), also arm
  `os_fade_remaining = os_fade_len`.
- After decimation to the base-rate `dst_l`/`dst_r`, apply the raised-cosine
  gain 0→1 from [[0190]]'s `BypassXfade` weight (`rise = 0.5 - 0.5*cos(PI*t)`)
  as a single gain (no dry/wet — just scale the decimated output). Apply it
  **last**, after the 0107 mono→stereo R-seed, so it doesn't fight the seed
  ([lib.rs:364-368](../../vxn-1/crates/vxn-engine/src/lib.rs#L364-L368)).
- Decrement `os_fade_remaining` by the block length. `reset()` (transport reset)
  leaves it at 0 — a transport reset doesn't change the factor, so no fade.

Reuse the raised-cosine weight from `BypassXfade` (extract the pure weight fn if
it's cleaner than instantiating a full `BypassXfade` for a gain-only ramp).

## Acceptance criteria

- [x] An oversampling factor change (1↔2↔4↔…) crossfades the output over ~5 ms
      after the decimator reset; the hard step at the switch is removed (join
      `d4` ~1.5e-2 vs ~1.2 raw reset — ~80× better). See design-change note.
- [x] The fade is applied after the mono→stereo R-seed and does not disturb L/R
      phase alignment (0107 behaviour intact) — same weights on both channels,
      applied last in `decimate_block`.
- [x] No fade on a transport `reset()` (factor unchanged); no per-sample cost
      when `os_fade_remaining == 0`.
- [x] Latency still unreported; no host-restart on OS change (unchanged).
- [x] `cargo test -p vxn-engine` green.

## Close-out

**Design change — crossfade-from-hold, not fade-in-from-zero.** The ticket's
premise (a fade-in from zero buries the post-reset FIR settle) turned out not to
hold: measurement shows the decimator reset makes the new FIR emit near-zero for
its first sample, so the output *steps down* from the pre-switch level at the
switch. A gain fade-in from zero doesn't hide that step — it *is* the step
(verified: raw reset and reset+fade-in gave an identical join `d4` ≈ 1.2). Only a
fade-*in* can't fix a step-*down*.

Implemented instead: `OutputStage` tracks `prev_last_l/r` (the last emitted
sample each block) and, on a genuine factor change, snapshots it into
`os_hold_l/r`. Over the ~5 ms window (`OS_FADE_MS = 5.0`) the decimated output is
crossfaded from that frozen level into the rebuilt output —
`(1−rise)·hold + rise·new` — so the join is continuous (first sample = hold =
previous sample) and the FIR settle lands under low weight. Join `d4` drops to
~1.5e-2. A residual first-sample *slope* kink remains (the held level is flat, the
pre-switch waveform was sloping) — ~80× below the raw click, sub-perceptual, but
non-zero; a fully click-free fix would need the old-rate signal to continue,
which isn't available without dual-rate rendering (rejected as too heavy). Final
audibility check deferred to the Reaper listen ([[0192]]).

Reuses [[0190]]'s `raised_cosine_rise` weight and `ms_to_samples`. Priming
(`os_primed`) adopts the first factor with no reset/fade (decimators already
empty), mirroring 0190's fade prime, so boot at a non-1× default doesn't glitch.

## Notes

- Depends on [[0190]] for the raised-cosine weight helper.
- Rationale for fade-in-only vs crossfade: the switch happens at a block boundary
  when the param changes, so the outgoing audio can't be pre-faded; the incoming
  FIR-settle transient is the only discontinuity, and a fade-in from zero covers
  it. Related: [[vxn1-silent-skip-filter-state]].
- Verify in Reaper via [[0192]] / [[verify-audio-in-reaper]]; tune ~5 ms if the
  settle pokes through.
