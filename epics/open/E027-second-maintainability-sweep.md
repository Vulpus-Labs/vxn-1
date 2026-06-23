---
id: E027
product: monorepo
title: Second maintainability sweep — shared-primitive dedup, engine decomposition, plumbing tax
status: open
created: 2026-06-23
---

## Goal

Remediate the findings of the 2026-06-23 second maintainability
sweep (six Sonnet area reviews — DSP, engine, shared-core,
CLAP, web-JS, web-controller — plus two deep-review passes and
hand verification). vxn-3 was excluded from the sweep by
request.

The architecture is sound: no UB-class defects, no hidden
bugs (both correctness suspects were deep-reviewed and
withdrawn — see Notes), the audio threads stay
allocation-free, the param models are single-source. This
epic is therefore not a redesign. It pays down the debt that
the sweep found genuinely impedes feature velocity, in
priority order:

1. **The dominant theme — vxn-2 forks shared primitives.**
   `vxn2-dsp` has an empty `[dependencies]`; `vxn2-engine`
   pulls `vxn-core-app` but not `vxn-core-utils`. So vxn-2
   carries hand-maintained copies of smoothing, flush-to-zero,
   midi-pitch, tempo-sync subdivisions, the limiter, the
   half-band oversampler, and the scalar tanh — each a
   "fix both or silently drift" trap. None are SIMD hot-loop
   code; all are safe to share. Highest win/effort ratio.

2. **Engine god-method + plumbing tax.**
   `vxn2-engine` `process_block` is a 530-line, 12-stage
   serial block, the `Engine` struct carries five lockstep
   parallel ramp `Vec`s, and the blob-migration ladder costs
   five coordinated edits per appended param with no
   compile-time check.

3. **Cross-synth web duplication + god-files.**
   The value-pop singleton, cutoff-tuned math, and the
   pointer-drag lifecycle are each re-implemented per synth
   with observable divergences; `op-row.js` (842) and
   `panels.js` (1187) own too much.

4. **Lower-urgency dedup + hygiene.**
   vxn2-clap re-copies vxn-core-clap helpers; the web
   controller has a god struct and a duplicated NaN-diff
   loop; the preset codec is copied vxn-1↔vxn-2; plus a
   batch of small items the sweep enumerated.

## In scope

- Make `vxn2-dsp` / `vxn2-engine` depend on
  `vxn-core-utils` and delete the forked `Smoothed` /
  `one_pole_coeff` / `ms_to_samples` (`smoother.rs`),
  `ScopedFlushToZero` (`ftz.rs` + the third inline copy in
  `phaser.rs`), `midi_to_hz` (`op.rs`), and the
  `SUBDIVISIONS` / `Subdivision` / `index_from_norm` table
  (`lfo.rs`, plus the vxn-1 `vxn-app/sync.rs` copy).
- Promote `PeakWindow` / `LimiterCore` / `StereoLimiter`
  (+ `DelayLine`), the `HalfbandFir` / `Oversampler`
  decimation half, and the branched-scalar `fast_tanh` into
  `vxn-core-utils`; both synths consume.
- Decompose `vxn2-engine` `process_block` (extract the
  16-stack matrix loop into `cook_stacks_block`) and
  collapse the parallel ramp `Vec`s into one
  `Vec<RampState>`.
- Replace the bespoke per-version blob-migration seed blocks
  with a data-driven migration table, and add per-section
  `const`-asserts that the `OFF_*` offsets still match their
  descriptor ids.
- Extract the shared `stack_tick_stereo`/`stack_tick_mono`
  hot loop into a single `tick_ops` kernel (Stack struct
  split is a stretch goal — design-review gated).
- Have `vxn2-clap` consume `vxn-core-clap`'s `batch_range`,
  the MIDI/note portion of `dispatch_event`, and the
  gesture-bracket decision; add a controller-construction
  factory fn shared by production and tests.
- Lift the value-pop singleton, cutoff-tuned math, and the
  `wireDrag` pointer-drag lifecycle into a shared web util;
  both synths consume.
- Split `op-row.js` (KS-graph, EG-graph, algo-data) and
  re-shape `panels.js` toward vxn-2's modular `panels/`
  layout.
- Extract `ControllerState` staging buffers / param mirror
  into sub-structs and dedup the `pump_readback` /
  `diff_params` NaN-diff loop into one helper.
- Extract the shared preset-codec scaffold
  (`value_for`, `Meta`, `PresetError`, sparse-TOML) into a
  shared home.
- Hygiene batch: delete the dead `EditorBackend::open` trait
  method, add the xorshift cross-reference comment, document
  the `SharedParams` threading guarantee.

## Out of scope

- vxn-3 — excluded from this sweep by request.
- **The operator/EG level curve (`EG_LOG_LEVELS`, `eg.rs`,
  `op.rs` cook level).** Concurrently owned by epic **E026**
  (DX7-faithful level curve). This epic does not touch the EG
  curve; the hygiene ticket explicitly defers it.
- New features, perf work, or audio-behaviour changes; every
  ticket is behaviour-preserving (render baseline unchanged)
  except the blob-migration ticket, which changes no values,
  only the codec shape.
- A full `EditorBackend` trait redesign — the deep review
  found the trait is never dispatched (one impl, zero
  callers); only the dead `open` method is deleted.
- The `vxn2-clap` inactive-flush path — deep review showed
  the suspected stale-value bug is fully masked by the
  all-ones dirty seed; no change (see Notes).

## Phasing

- **0117** vxn-2 consumes `vxn-core-utils` (smoothing/ftz/midi/sync).
- **0118** Promote limiter + half-band + scalar-tanh to `vxn-core-utils`.
- **0119** `vxn2-engine` `process_block` extract + ramp-`Vec` collapse.
- **0120** `vxn2-engine` blob-migration table + section const-asserts.
- **0121** vxn-2 `tick_ops` kernel extraction (Stack split stretch).
- **0122** `vxn2-clap` consumes `vxn-core-clap`.
- **0140** Shared web widgets (value-pop / cutoff-math / wireDrag).
- **0141** Web god-file splits (op-row.js, panels.js).
- **0142** `vxn-web-controller` `ControllerState` split + NaN-diff dedup.
- **0143** Shared preset-codec scaffold.
- **0144** Second-sweep hygiene batch.

(Tickets 0123–0128 belong to the concurrent E026 level-curve
epic, which is still allocating numbers — this epic's second
half jumps to 0140 to stay clear of that range.)

## Dependency order

```text
0117 (consume core-utils)  ── land first; cheap, unblocks confidence
0118 (promote to core)     ── after/with 0117 (both touch vxn-core-utils)
0119 (process_block)       ── independent (vxn-2 engine)
0120 (blob migration)      ── independent; add round-trip test first
0121 (tick_ops)            ── independent; SIMD-sensitive, asm-verify
0122 (vxn2-clap reuse)     ── independent
0140 (shared web util)     ── before 0141 (splits consume the util)
0141 (web file splits)     ── after 0140 and after JS tests exist
0142 (controller split)    ── independent (vxn-1)
0143 (preset codec)        ── independent; low
0144 (hygiene)             ── last; touches many files, avoid conflicts
```

## Tickets

| # | Ticket | Product | Priority |
|---|--------|---------|----------|
| 1 | [0117 — vxn-2 consume vxn-core-utils](../../tickets/open/0117-vxn2-consume-core-utils.md) | vxn-2 | high |
| 2 | [0118 — Promote limiter/halfband/tanh to core](../../tickets/open/0118-promote-dsp-primitives-to-core.md) | monorepo | high |
| 3 | [0119 — process_block extract + ramp collapse](../../tickets/open/0119-engine-process-block-decompose.md) | vxn-2 | high |
| 4 | [0120 — blob-migration table](../../tickets/open/0120-blob-migration-table.md) | vxn-2 | high |
| 5 | [0121 — tick_ops kernel extraction](../../tickets/open/0121-stack-tick-ops-extract.md) | vxn-2 | medium |
| 6 | [0122 — vxn2-clap consume core-clap](../../tickets/open/0122-vxn2-clap-consume-core.md) | vxn-2 | medium |
| 7 | [0140 — shared web widgets](../../tickets/open/0140-shared-web-widgets.md) | monorepo | medium |
| 8 | [0141 — web god-file splits](../../tickets/open/0141-web-god-file-splits.md) | monorepo | medium |
| 9 | [0142 — controller split + NaN-diff dedup](../../tickets/open/0142-controller-state-split.md) | vxn-1 | medium |
| 10 | [0143 — shared preset-codec scaffold](../../tickets/open/0143-preset-codec-scaffold.md) | monorepo | low |
| 11 | [0144 — second-sweep hygiene batch](../../tickets/open/0144-second-sweep-hygiene.md) | monorepo | low |

## Acceptance

- Exactly one copy of each shared primitive in the workspace:
  `Smoothed`, `one_pole_coeff`, `ms_to_samples`,
  `ScopedFlushToZero`/`flush_denormal`, `midi_to_hz`, the
  tempo-sync subdivision table, `LimiterCore`/`StereoLimiter`,
  `HalfbandFir`/`Oversampler`, and the scalar `fast_tanh`.
  `grep` across `crates/`, `vxn-1/crates/`, `vxn-2/crates/`
  finds one definition each (the branchless poly-lane tanh
  stays separate by design and is documented as such).
- `cargo test --workspace` green at epic close; every
  behaviour-preserving ticket leaves its product's
  `tests/baseline.rs` render hash unchanged. 0120 adds a
  round-trip test loading every historical blob version
  (1..=current) and asserting identical store contents
  before/after the table rewrite.
- 0119 removes ≥300 lines from `process_block`; adding a new
  mod-ramp type touches one `RampState` field, not five
  parallel `Vec`s.
- 0120 reduces "append one param" from five coordinated edits
  to one migration-table row; a section-offset drift fails to
  compile.
- vitest green for 0140/0141; the value-pop, cutoff-tuned
  math, and pointer-drag lifecycle each have one shared
  implementation with direct test coverage.
- No `process_block` / `cook_stacks_block` stage reordering
  changes the rendered baseline (0119 ships the 12-stage
  ordering doc table).

## Notes

Source: six Sonnet area reviews + two deep-review passes
(2026-06-23). Where line numbers drift from HEAD, symbol
names are authoritative.

**Coexists with E026** (DX7-faithful level curve), opened the
same day against the same uncommitted working tree. E026 owns
the EG/operator level curve; this epic owns everything else
and must not touch `eg.rs` / `op.rs` cook-level code. Ticket
ranges are disjoint (E026: 0123–0128; E027: 0117–0122,
0140–0144).

**Two claims withdrawn during deep review** — do not re-open
them in a future sweep:

1. *vxn2-clap inactive-flush stale value* — suspected that
   `PluginMainThreadParams::flush` writing straight to
   `shared.params.set()` (bypassing the controller) would
   show a stale value on GUI open. REAL-BUT-MASKED:
   `SharedParams::new` seeds `dirty_values` all-ones and the
   instance lives for the whole plugin lifetime, so the first
   GUI tick always broadcasts a full snapshot. The vxn-2 test
   at `vxn2-clap/lib.rs:1439` already pins this. No fix.

2. *`EditorBackend::open` Liskov footgun* — HARMLESS VESTIGE:
   one impl (`WebEditor`), zero callers, never trait-
   dispatched (every synth calls the concrete free function
   `open_editor`). 0144 deletes the dead method; no trait
   redesign needed.

**SIMD discipline.** 0121 touches the SoA stack kernel.
Do not merge the branchless poly-lane tanh into the scalar
one — the split is deliberate (memory
`vxn1-tanh-branchless-only`). Verify 0121 with an asm dump of
the post-LTO kernel, not per-crate asm (misleading pre-LTO —
memory `vxn1-ota-filter-perf`); confirm NEON `.4s` survives
(grep pitfall — memory `vxn1-neon-grep-pitfall`). Adding
factory/preset TOMLs won't recompile — touch `factory.rs`
before `xtask install` (memory `vxn2-include-dir-no-rerun`).

Stage explicit paths when committing — `git add -A` pollutes
commits with concurrent vxn-2 working-tree churn (memory
`vxn-concurrent-vxn2-work-no-git-add-all`).
