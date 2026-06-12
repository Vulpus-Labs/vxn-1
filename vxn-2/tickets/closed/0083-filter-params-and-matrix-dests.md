---
id: "0083"
title: "Filter params + Cutoff/Resonance matrix destinations"
priority: high
created: 2026-06-12
epic: E007
depends: []
---

## Summary

Fourth ticket of [E007](../../epics/open/E007-optional-per-voice-filter.md).
Expose the filter to patches: a Filter parameter section plus two new mod matrix
destinations so cutoff and resonance are per-voice modulatable (ADR 0004 §7, §9).
This ticket only adds the param/matrix *surface* — wiring it into the render path
is [0084](0084-per-stack-filter-render-path.md).

## Design

New **Filter** section in [params.rs](../../crates/vxn2-engine/src/params.rs) and
`PARAMETERS.md`, appended after Master (ID stability is not a constraint —
`id-stability-dropped`):

- `filter-enable` — bool, default **off**.
- `filter-cutoff` — Hz, log taper (e.g. 20 Hz .. 20 kHz). Matrix dest `Cutoff`.
- `filter-resonance` — `[0, 1]`, self-osc at 1. Matrix dest `Resonance`.
- `filter-mode` — enum LP / HP / BP / Notch.
- `filter-slope` — enum 2-pole / 4-pole.
- `filter-drive` — input drive into stage-0 `tanh` (≥ 0).
- `filter-oversample` — enum 1× / 2× / 4× / 8×.

CLAP exposure: `cutoff`, `resonance`, `drive` are automatable continuous
controls; `enable`, `mode`, `slope`, `oversample` are structural/topology
selectors — excluded from CLAP automation (same rationale as `algo` and matrix
source/dest), patch state only.

Matrix ([matrix.rs](../../crates/vxn2-engine/src/matrix.rs)):

- Add `DestId::Cutoff` and `DestId::Resonance` to the `DestId` enum and its
  `from_u32` / name tables; both **pitch-unshaped** scalar dests.
- Both resolve to a **per-stack scalar** (ADR 0004 §7): per-lane contributions
  collapse via lane-0 (start simple; active-lane mean is the documented
  fallback if a patch wants spread-driven cutoff). Mirror the existing per-stack
  aggregation already used for `DelayMix` / `ReverbMix`.
- `Cutoff` modulation is applied in the cutoff's natural domain (octaves/log),
  not linear Hz, so a fixed depth is musically uniform across the range.

Update `PARAMETERS.md` counts (Filter section + new subtotal) and the matrix
dest enumeration.

## Acceptance criteria

- [x] Seven filter params present with correct ranges/tapers/defaults;
  `filter-enable` defaults off. (Cutoff 20–20 kHz Exp/mid 1k, default 12 kHz;
  reso 0–1 default 0; mode LP; slope 4-Pole; drive 0.1–16 Exp default 1; OS 4×.)
- [x] ~~`cutoff`/`resonance`/`drive` are CLAP params; `enable`/`mode`/`slope`/
  `oversample` are patch-state-only selectors.~~ **Design change (user-approved):
  all 7 are CLAP params.** The codebase has no automation-exclusion flag
  ([vxn2-clap/src/lib.rs:530](../../crates/vxn2-clap/src/lib.rs) flags every
  PARAMS entry `IS_AUTOMATABLE`) and no general non-CLAP persistence channel
  (ks curves aren't persisted; only matrix topology has a bespoke path). The
  selectors follow the existing `delay-on`/`algo`/`lfo2-shape` precedent —
  structural CLAP params. `filter-enable` is the exact analogue of `delay-on`.
- [x] `DestId::Cutoff` / `DestId::Resonance` added with name/label/gain-table
  entries and `from_u8` round-trip; dest-idx test extended; v6→v7 blob
  migration test added (`load_bytes_migrates_v6_param_layout`).
- [x] Cutoff modulation applied in log/octave domain — `DEST_GAIN[Cutoff] = 8.0`
  octaves; dest value is octaves, consumer (0084) applies `cutoff·2^value`.
- [x] `PARAMETERS.md` updated: Filter section, subtotals (now 186 — also
  corrected stale per-op/dest counts), matrix dest list (29 dests).
- [x] Default patch (filter off) round-trips through preset save/load unchanged
  (`default_patch_round_trips_through_text`, `snapshot_bytes_round_trip_*`).

## Notes

The render path is not touched here — with `filter-enable` off these params are
inert and the matrix dests resolve to values nothing consumes yet.
[0084](0084-per-stack-filter-render-path.md) is what reads them. Keeping this
ticket render-free lets it land in parallel with the DSP ports
([0080](0080-port-ota-ladder-kernel.md)/[0081](0081-port-halfband-decimator.md)).
