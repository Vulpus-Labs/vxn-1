---
id: E035
product: vxn-1
title: "vxn-1 toggle declick — glitch-free FX on/off + oversampling change"
status: open
created: 2026-07-07
---

> **Port vxn-2's glitch protection to vxn-1.** vxn-2 crossfades the filter
> toggle (8 ms raised-cosine, equal-gain) and deliberately avoids latency
> reporting so an oversample change can't force a host restart. vxn-1 has
> neither: FX on/off is a hard switch and an oversample change hard-resets the
> decimators — both audibly click. This epic brings the two paths up to vxn-2's
> standard.

## Goal

Kill the audible discontinuities in vxn-1 when the user toggles a master FX or
changes the output oversampling factor. When this epic closes:

- Toggling **phaser / chorus / delay / reverb** (and re-engaging the limiter)
  crossfades between dry and wet over a short raised-cosine window instead of
  hard-switching — no click on either edge.
- Changing the **oversampling factor** no longer clicks: the FIR-settle
  transient after the rate-specific decimator reset is buried under a short
  fade-in.
- The steady-state hot path is **unchanged** — no per-sample cost added when
  nothing is toggling, and the engine stays sample-exact against a build with
  the effect absent when the fade is idle (the current fast-path guarantee at
  [lib.rs:242-244](../vxn-1/crates/vxn-engine/src/lib.rs#L242-L244)).

## Why now

The synths are in polish. Toggle clicks are the kind of rough edge that reads as
"unfinished" the first time a user flips an effect in a DAW. vxn-2 already
solved the same two problems and the mechanism is small and well-understood —
this is cheap parity, not new DSP.

## Background — where the clicks come from

- **FX toggle = hard switch, no protection.** Phaser/chorus/delay/reverb
  copy-through when off and snap on with no crossfade
  ([lib.rs:258-306](../vxn-1/crates/vxn-engine/src/lib.rs#L258-L306)); the on/off
  params are `Glide::Snap`
  ([smoothing.rs:113-131](../vxn-1/crates/vxn-engine/src/smoothing.rs#L113-L131)).
  Only the limiter resets on its off→on edge
  ([lib.rs:300-306](../vxn-1/crates/vxn-engine/src/lib.rs#L300-L306)).
- **OS change = hard decimator reset.** `on_os_change` zeroes both rate-specific
  FIR decimators when the factor moves
  ([lib.rs:356-362](../vxn-1/crates/vxn-engine/src/lib.rs#L356-L362)); the FIR
  then settles from zero → a transient at the switch.
- **Latency is already unreported** — vxn-1 declares no CLAP `PluginLatency`
  extension, so (unlike a naïve fix) an OS change does *not* force a host
  restart. We keep it that way; matches vxn-2's 0086-reverted decision.

## Design decisions (locked)

- **Per-FX bypass crossfade, not whole-chain dual-render.** vxn-2 dual-renders
  dry-vs-filtered from one stack tick; that works because the two paths share no
  stateful DSP that both advance. vxn-1's FX carry delay lines and LFOs —
  rendering old-flags and new-flags both would double-advance that state. So each
  stage instead crossfades its *own* dry input against its wet output.
- **Equal-gain raised-cosine** weight `rise = 0.5 − 0.5·cos(π·t)`, `t∈[0,1]`,
  `w_dry + w_wet = 1`. Zero slope at both endpoints (no corner click); equal-gain
  (not equal-power) because dry and wet are strongly correlated — same rationale
  vxn-2 documents at `FILTER_XFADE_MS`.
- **OS change: fade-in only.** The pre-switch audio is already continuous; only
  the post-reset FIR settle clicks. A ~5 ms raised-cosine gain ramp 0→1 on the
  decimated output covers it. No dual-rate crossfade (would need voices rendered
  at two OS rates — too heavy for a rare user action).
- **Fade lengths:** ~10 ms FX, ~5 ms OS. (Provisional — tune during
  [[verify-audio-in-reaper]].)

## Planned tickets

Dependency chain: **0190 → 0191 → 0192**. (0190 lands the shared raised-cosine
helper that 0191 reuses; 0192 verifies both.)

- [ ] **0190** — **FX toggle declick.** Shared `BypassXfade` raised-cosine helper
      + apply it to phaser / chorus / delay / reverb; keep the limiter's
      reset-on-edge and add its bypass fade. Steady-state fast path preserved.
- [ ] **0191** — **Oversampling-change declick.** Raised-cosine fade-in on the
      decimated output after the decimator reset in `on_os_change`. Reuses the
      0190 helper.
- [ ] **0192** — **Declick regression tests + DAW verify.** Offline no-step
      assertions across each toggle and an OS change; sample-exact-when-idle
      guard; manual Reaper listen.

## Risks

- **Idle CPU.** The wet path must run *during* a fade even when the flag says
  off. Gate it: compute wet only when `flag_on || fade_active`, so once an
  on→off fade completes the stage returns to the zero-cost passthrough. Verify
  idle profile against [[vxn1-render-loop-optimized]] — no steady-state
  regression.
- **Fast-path / sample-exact guarantee.** The passthrough copy at
  [lib.rs:260-263](../vxn-1/crates/vxn-engine/src/lib.rs#L260-L263) is what keeps
  the engine bit-exact vs an effect-absent build. The fade must be *fully idle*
  (remaining == 0, flag off) before that copy path is taken, or the guarantee and
  its tests break.
- **OS fade vs mono→stereo seed.** The OS fade-in and the 0107 mono→stereo R
  decimator seed both touch the decimated block at the boundary; apply the fade
  *last* so it doesn't fight the seed. Related: [[vxn1-silent-skip-filter-state]].
- **Reverb double dry/wet.** Reverb already does an internal dry/wet mix; the
  bypass fade wraps *around* it (crossfades the reverb's output bus against the
  reverb's input). Don't confuse the two mixes.

## Acceptance

- No audible click when any of phaser/chorus/delay/reverb/limiter is toggled, or
  when the oversampling factor is changed, verified in Reaper.
- Offline tests: no output sample step above threshold across each toggle edge
  and across an OS change; engine stays bit-exact when no toggle/OS change is in
  flight (fast-path guard).
- No per-sample cost added to steady-state render; idle profile unchanged.
- `clap-validator` 0 failures; `cargo test -p vxn-engine` green.
