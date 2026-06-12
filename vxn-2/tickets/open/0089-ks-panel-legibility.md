---
id: "0089"
title: "KS panel legibility: note-name readout, label what each control scales, surface the rate pivot"
priority: medium
created: 2026-06-12
epic: null
depends: []
---

## Summary

The keyboard-scaling (KS) control on the op row is functional but doesn't
communicate **what it scales**, **where the split lands on the keyboard**, or
**in what units** — so it's effectively unreadable. Three distinct confusions,
all UX not DSP:

1. **No note-name readout.** The break point is a raw MIDI int (0..127). The
   graph draws octave gridlines but never labels them, and dragging the BP
   handle shows no note name ([op-row.js:493-496](../../crates/vxn2-ui-web/assets/panels/op-row.js#L493)).
   You can't tell what key the split lands on.

2. **No indication what KS affects.** There are *two* mechanisms with *different
   pivots*, and the panel hides this:
   - **Level scaling** ([ks.rs:32](../../crates/vxn2-dsp/src/ks.rs#L32)) —
     multiplies per-op `level` (carrier → loudness; modulator → mod-index /
     brightness) across the keyboard. Pivot = the `ks-break-pt` you drag. L
     depth/curve below, R depth/curve above.
   - **Rate scaling** ([ks.rs:72](../../crates/vxn2-dsp/src/ks.rs#L72)) —
     multiplies all four EG rates (higher notes → shorter envelopes). Separate
     `ks-rate` knob, **pivot hardcoded at A3 (note 57), NOT the break point.**
   The graph only depicts level scaling; rate is a bare `KsRt` fader
   ([op-row.js:594](../../crates/vxn2-ui-web/assets/panels/op-row.js#L594))
   with no visual link, no pivot marker, and nothing saying it touches envelope
   timing rather than level.

3. **The graph misrepresents the curve.** Both sides are drawn as straight
   lines ([op-row.js:450-451](../../crates/vxn2-ui-web/assets/panels/op-row.js#L450)),
   but the DSP supports an exponential shape `(d/4)²`
   ([ks.rs:51-57](../../crates/vxn2-dsp/src/ks.rs#L51)). The curve type is a
   four-way enum (Neg/Pos × Lin/Exp) that is currently **frozen** in code with
   no control and no persistence (see *Deferred sub-task* below) — so the graph
   can neither show nor set the real shape.

## Design

UX-first. No DSP change required for items 1-2.

1. **Note-name readout + axis labels.**
   - Label the octave gridlines with note names (`C1`, `C2`, …) — they're
     already drawn at `oct * 12` ([op-row.js:415-418](../../crates/vxn2-ui-web/assets/panels/op-row.js#L415)).
   - While dragging the BP handle, show a live readout of the note name
     (MIDI → name helper; reuse whatever the keyboard/MTS code already has, or
     a small `noteName(midi)` util). Same for L/R handles → show the level
     multiplier at the keyboard extreme (e.g. "−12 dB @ C7").
   - Label the Y axis: boost above centre, cut below; unity at the BP line.

2. **Say what scales, and surface the rate pivot.**
   - Title/legend the graph as **Level scaling**.
   - Either draw the rate-scaling pivot (A3) as a second marker on the same
     keyboard axis with its own legend ("Rate ×, pivots A3"), or label the
     `KsRt` fader so it's unambiguous it affects **envelope speed**, not level,
     and pivots independently at A3. Prefer the on-graph A3 marker — one
     keyboard axis, two annotated pivots, no hidden model.

3. **Curve shape (depends on the deferred sub-task).** Only once curves are
   controllable + persisted: draw the real shape (quadratic for Exp) and add a
   per-side Lin/Exp · boost/cut selector. Until then, keep the straight-line
   draw but **annotate it as the fixed Neg-Lin / Neg-Exp default** so it doesn't
   read as the literal response.

## Deferred sub-task — persist + expose KS curves (was the original 0089)

The two per-op curve enums `ks_l_curve` / `ks_r_curve`
([op.rs:49-50](../../crates/vxn2-dsp/src/op.rs#L49)) are hardcoded in `read_op`
([shared.rs:978-979](../../crates/vxn2-engine/src/shared.rs#L978)) — read from
nowhere, persisted nowhere, despite the misleading `// not CLAP — preset state`
comment. **Persistence alone is pointless** (you'd round-trip a constant), so
this is gated on giving curves a real control (design item 3).

When/if we do it (non-CLAP, no automation — owner's call), mirror the mod-matrix
non-CLAP persistence exactly:

- Pack 12 curves (6 ops × 2 sides × 2 bits) into one `AtomicU32` in
  `SharedParams` beside `matrix_meta`
  ([shared.rs:264](../../crates/vxn2-engine/src/shared.rs#L264)); accessors like
  `matrix_row_raw` / `set_matrix_row_raw`, flip the dirty bit on write.
- Bump `BLOB_VERSION` ([shared.rs:115](../../crates/vxn2-engine/src/shared.rs#L115));
  append the u32 after the matrix trailer in `snapshot_bytes`
  ([shared.rs:559](../../crates/vxn2-engine/src/shared.rs#L559)); length-tolerant
  read for old blobs → legacy NegLin/NegExp default.
- Write each curve as a name-keyed `op{n}-ks-l-curve` label (`"neg-lin"` …) in
  the preset `params` table (sparse, no schema bump — additive).
- Thread the curve into `read_op` via a `ParamView` accessor instead of the
  hardcode; the throwaway-`SharedParams` preset impls
  ([preset.rs:193](../../crates/vxn2-engine/src/preset.rs#L193),
  [shared.rs:193](../../crates/vxn2-engine/src/shared.rs#L193)) extend the same
  way the matrix accessors did.

Until then: delete or correct the `// not CLAP — preset state` comment so it
stops implying wiring that doesn't exist.

## Acceptance criteria

- [ ] Octave gridlines on the KS graph carry note-name labels.
- [ ] Dragging the BP handle shows the live note name; dragging L/R shows the
  resulting level multiplier (dB) at the keyboard extreme.
- [ ] The graph is clearly labelled as **level** scaling, and the rate
  mechanism's independent **A3** pivot is visible/annotated (not hidden inside a
  bare fader).
- [ ] The straight-line draw is either made accurate (curve sub-task done) or
  annotated as the fixed default so it doesn't misrepresent the response.
- [ ] No DSP behaviour change for items 1-2 (pure UI).
- [ ] (Sub-task, if pursued) KS curves round-trip through preset + host state;
  old blobs migrate to legacy defaults; `read_op` no longer hardcodes them.

## Notes

The persistence concern that opened this ticket turned out to be downstream of a
legibility problem: the curves can't be set, so saving them is moot. Fixing the
panel's communication (what/where/units) is the real win; curve
control+persistence is a clean follow-on that only pays off once a control
exists. Persistence pattern mirrors the mod matrix — see [[vxn2-preset-system]],
[[vxn2-architecture]], [[vxn2-mvc-discipline]] (view never reads model; drag
gated locally). Independent of the filter epic (E007).
