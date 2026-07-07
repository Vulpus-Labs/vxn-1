---
id: "0192"
product: vxn-1
title: "Declick regression tests + DAW verify — FX toggles and OS change"
priority: medium
created: 2026-07-07
epic: E035
depends: ["0190", "0191"]
---

## Summary

Lock in the [[0190]] / [[0191]] declick work with offline regression tests and a
manual Reaper listen. Two guarantees to pin: (1) no output discontinuity across a
toggle edge or OS change, and (2) the engine stays bit-exact when nothing is in
flight (the fast-path guarantee that the fades must not break).

## Design

**Offline tests — [vxn-engine tests](../../vxn-1/crates/vxn-engine/tests)**

- **No-step across toggles.** For each of phaser/chorus/delay/reverb/limiter:
  render a steady tone, flip the flag mid-buffer, assert the max
  sample-to-sample delta around the edge stays below a threshold (no hard step).
  Run both edges (off→on and on→off). Model the assertion on any vxn-2 declick
  test if one exists; otherwise a plain `abs(y[i] - y[i-1]) < eps` sweep over the
  fade window.
- **No-step across OS change.** Render a tone, change the oversampling factor
  mid-stream, assert the same no-step bound over the fade-in window.
- **Bit-exact when idle.** With no toggle and no OS change in flight, assert the
  output is identical to a reference render — the passthrough fast path must be
  untouched by the added fade machinery (guards the sample-exact-vs-absent
  contract at
  [lib.rs:242-244](../../vxn-1/crates/vxn-engine/src/lib.rs#L242-L244)).

**DAW verify**

- Manual listen in Reaper per [[verify-audio-in-reaper]] — do **not** build a
  headless audio harness. Toggle each FX and sweep the OS factor while a pad
  sustains; confirm no click. Tune the ~10 ms FX / ~5 ms OS windows if anything
  pokes through.

## Acceptance criteria

- [x] No-step tests pass for all five FX toggles (both edges) and for an OS
      factor change (`tests/declick.rs`).
- [x] Bit-exact-when-idle test passes (fast path unchanged) —
      `all_fx_off_is_bit_exact_across_fx_params` + `baseline_render_is_stable`.
- [x] `cargo test -p vxn-engine` green. `clap-validator`: not re-run (no
      host/param-surface change; engine-internal only) — worth a pass before the
      epic closes.
- [ ] Reaper listen confirms no audible click on any toggle or OS change; final
      fade lengths recorded here. **Pending — manual, [[verify-audio-in-reaper]].**

## Close-out (offline portion)

`tests/declick.rs` added. The metric that isolates the *switch* from an effect's
own DSP is the `d4` straddling the edge sample (the "join"); the assertion is
that the join stays within a small factor of the steady-state tone, which a hard
switch blows by ~3 orders of magnitude (proven in-repo: a forced hard phaser
switch gives join ~2.3e-1 vs the crossfade's ~1.6e-4). Steady baseline uses the
worst `d4` over the *whole* settled plateau, since vxn-1's FX are LFO-modulated
and a short window undersamples their own slew.

Tests: `phaser/chorus/delay/reverb/limiter_toggle_is_click_free` (both edges),
`oversampling_change_is_declicked` (join ≪ raw-reset click + absolute ceiling),
`all_fx_off_is_bit_exact_across_fx_params`.

Documented eps / notes:

- FX join clean at `k = 4×` steady for all five effects.
- OS-change asserts join `< raw_reset/10` and `< 5e-2` (measured ~1.5e-2); the
  residual is the slope kink from the unavoidable decimator-state discontinuity
  ([[0191]] close-out) — flag for the Reaper listen.
- Effect cold-start onset (chorus/delay fill, limiter attack) is effect-inherent,
  not a toggle click, and is deliberately *not* asserted on ([[0190]] close-out).

**Remaining:** Reaper listen + fade-length tuning (~10 ms FX / ~5 ms OS
provisional) and a `clap-validator` pass before [[E035]] closes.

## Notes

- Closes epic [[E035]] together with [[0190]] and [[0191]].
- If a threshold proves flaky (e.g. reverb tail energy), assert on the *fade
  window* delta rather than absolute level, and document the chosen eps.
