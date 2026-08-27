---
id: "0227"
product: monorepo
title: "Pure-move kernels into vxn-core-dsp: DynamicsBlock, HpfKernel, scalar OTA ladder + OtaLadderCoeffs"
priority: high
created: 2026-08-02
epic: E040
depends: ["0224", "0226"]
---

## Summary

Final ticket of [E040](../../epics/open/E040-vxn-core-dsp-foundations.md).
Move the three kernels that are already duplicates:

- **DynamicsBlock** — vxn-1's
  [dynamics.rs](../../vxn-1/crates/vxn-dsp/src/dynamics.rs) header says
  "Ported verbatim from vxn-2"; measured diff is import path + test helpers
  only, kernel byte-identical. Move to `vxn-core-dsp::dynamics`; both
  `vxn-dsp/src/dynamics.rs` and
  [vxn2-dsp/src/dynamics.rs](../../vxn-2/crates/vxn2-dsp/src/dynamics.rs)
  become re-export shims.
- **HpfKernel** — TPT one-pole, effectively identical forks
  ([vxn-dsp/src/hpf.rs](../../vxn-1/crates/vxn-dsp/src/hpf.rs) vs
  [vxn2-dsp/src/hpf.rs](../../vxn-2/crates/vxn2-dsp/src/hpf.rs)). Move the
  scalar kernel; vxn-1's 8-wide `PolyHpf` stays in vxn-dsp (SoA body).
- **Scalar OTA ladder** — `OtaLadderKernel` / `OtaLadderCoeffs` /
  `FilterMode` / `FilterSlope` / mix tables from
  [vxn2-dsp/src/filter.rs](../../vxn-2/crates/vxn2-dsp/src/filter.rs) (+ the
  coefficient half of
  [vxn-dsp/src/ota_ladder.rs](../../vxn-1/crates/vxn-dsp/src/ota_ladder.rs)).
  `OtaLadderCoeffs::new` takes the 0226 `OsRate` newtype; `k_cap` stays
  absolute Hz with the rationale documented at the constructor.
  [poly/ladder.rs](../../vxn-1/crates/vxn-dsp/src/poly/ladder.rs) imports
  Coeffs/modes; its SoA body + `with_mix!` markers do NOT move.

## Acceptance criteria

- [ ] Three kernels in vxn-core-dsp, shims in place, zero import churn inside
      synth crates.
- [ ] DynamicsBlock's WetFade adoption (replacing its hand-rolled
      enabled/mix/mix_primed trio) lands as a **separate commit**, kept only
      if hashes stay byte-identical; else deferred to E041.
- [ ] vxn-1 baseline, vxn-1b parity, vxn-2 render hash, dynamics_integration,
      filter_integration, all bit-exact-passthrough tests: unchanged.
- [ ] asm-check: `poly/ladder` monomorph NEON counts unchanged; `filter_path`
      bench within noise.

## Notes

Ladder rate split preserved: coefficients cooked at OS rate, `tick_coeffs` at
base rate, `process` at OS rate — the 4-call protocol is untouched here
(E043/0237 later wraps its increments in `CoeffRamp`). Related:
[[vxn1-ota-filter-perf]], [[vxn1-soa-match-defeats-simd]].

## Close-out (2026-08-27)

Three kernels moved into `vxn-core-dsp`, all three synths bit-exact. **Two of
the ticket's three "already duplicates" claims were stale** — both were true when
it was written on 2026-08-02 and had drifted since.

### DynamicsBlock — vxn-1's copy was the superset, not a duplicate

The ticket says the measured diff is "import path + test helpers only, kernel
byte-identical". Not so: [0241](../closed/0241-dynamics-metering.md), closed
**2026-08-26**, added the gain-reduction metering tap (`gr_db_min`,
`take_gain_reduction_db`, a branch-free per-sample `min`) to vxn-1's copy and not
to vxn-2's fork. It is live — [vxn1b-engine/src/fx.rs:291](../../vxn-1b/crates/vxn1b-engine/src/fx.rs#L291)
publishes it as `MeterTap::DynamicsGr`. 0241's own close-out says it was "added
additively to the shared kernel ... so vxn-1 and vxn-2 are unaffected", which was
true of the *behaviour* but left the two files diverged.

So **vxn-1's version moved**, and vxn-2 adopts the superset. `gr_db_min` is a
side channel — never read back into the output — so vxn-2's audio is
bit-identical; it simply gains a tap it can publish whenever a meter wants one.
Both crates keep re-export shims.

### HpfKernel — identical bodies, different status

vxn-2's ships; vxn-1's was `#[cfg(test)]`-only, existing purely as the scalar
oracle its 8-wide `PolyHpf` is differentially tested against. Bodies matched, so
one kernel now serves both roles. `coeff` is exported alongside it: `PolyHpf`
keeps its own SoA body but must use the *same* cutoff mapping, and sharing the
kernel without the mapping would leave the oracle and the lane body free to
disagree on the one thing their differential test assumes they agree on.
`PolyHpf` itself stays in `vxn-dsp` (ADR 0002 §3).

### OTA ladder — the ladders had genuinely diverged, and were not unified

The ticket treats these as forks with mechanical deltas. They are not.
`OtaLadderCoeffs::new` differs in substance:

| | `k` |
|---|---|
| vxn-2 | `(4·resonance).min(k_cap(cutoff_hz))` |
| vxn-1 / vxn-1b | `4·resonance` — no cap |

`k_cap` is a cutoff-tracked feedback ceiling added 2026-06-12 as a sound-design
fix: the discrete ladder's self-oscillation threshold *falls* as cutoff rises, so
without it a high, matrix-modulated cutoff parks a screaming peak on FM's dense
inharmonic HF. Both are production paths —
[vxn-engine/src/voice.rs:1048](../../vxn-1/crates/vxn-engine/src/voice.rs#L1048),
[vxn1b-engine/src/bank.rs:676](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L676),
[vxn2-engine/src/engine.rs:2329](../../vxn-2/crates/vxn2-engine/src/engine.rs#L2329).

Unifying the constructor would have changed one synth's sound in either
direction, so this ticket's own "all hashes unchanged" criterion was
unsatisfiable as a pure move. **Split mechanism from policy instead** (user
decision, and what ADR 0002 §2's boundary test requires — sharing must not force
a signal-model compromise):

- **Shared** (`vxn-core-dsp::filter`): `OtaLadderKernel` (vxn-2's superset, with
  `state_abs_max` for its quiescence skip), `FilterMode` / `FilterSlope` / the
  mix tables, `compute_g`, and the `OtaLadderCoeffs` struct.
  `OtaLadderCoeffs::new` keeps vxn-1's **flat** `k`, so vxn-1 and vxn-1b call
  sites are untouched. `new_capped(.., max_k)` adds the ceiling as a *mechanism*.
- **Per-synth**: `K_CAP_BREAKS` + `k_cap` stay in `vxn2-dsp` — the breakpoint
  table is the voicing decision. vxn-2's production site and its 8 test sites now
  say `new_capped(.., k_cap(cutoff))` explicitly, which is behaviour-identical to
  before and makes the dependency visible instead of implicit.

vxn-1's `ota_ladder.rs` becomes a shim; its scalar kernel was a `#[cfg(test)]`
oracle like the HPF. `PolyOtaLadder`'s SoA body and `with_mix!` markers do not
move.

### Verification

- vxn-2 render hash **`0x533a37a7def1921a`** — unchanged.
- vxn-1 `baseline_render_is_stable` and all 7 declick tests pass unmodified.
- **asm-check: every watched symbol byte-for-byte identical to the 0223
  capture**, `RenderBank::render` included at 9632 — that symbol is where the
  ladder inlines, so it is the one that would have moved had the extraction cost
  vectorisation.
- `cargo test -p vxn-core-dsp -p vxn-dsp -p vxn2-dsp`: 39 / 91 / 165 green.

### Not done

`DynamicsBlock`'s `WetFade` adoption — the ticket asks for it as a **separate
commit, kept only if hashes stay byte-identical, else deferred to E041**. Not
attempted here: the move itself is already three kernels across five files, and
`WetFade`'s edge semantics differ from `DynamicsBlock`'s hand-rolled trio in
exactly the way [0226](0226-fxkernel-wetfade-control.md)'s close-out describes
(edge reported from `tick`, not `set_enabled`). Deferring it to E041 is the
ticket's own sanctioned outcome and keeps this one a pure move.
