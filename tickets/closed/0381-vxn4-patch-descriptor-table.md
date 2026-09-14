---
id: "0381"
product: vxn-4
title: "vxn-4 patch descriptor table — every field named, ranged, defaulted"
priority: high
created: 2026-09-10
epic: E052
---

## Summary

Give every field of a vxn-4 `Patch` a stable machine name, a descriptor (range,
taper, formatting, default) and an id. This is the prerequisite for both halves
of [E052](../../epics/open/E052-vxn4-editable-patches-and-faceplate.md): the
preset codec keys its TOML by these names, and the faceplate binds its controls
by them.

Today a patch is a Rust struct with no names at all —
[`Patch`](../../vxn-4/crates/vxn4-engine/src/patch.rs#L75) holds `ops`,
`routing`, `matrix`, `eg` and `gain`, and the only named surface anywhere in
vxn-4 is the eleven-entry CLAP table in
[vxn4-clap/src/params.rs](../../vxn-4/crates/vxn4-clap/src/params.rs).

**`ParamId` is not `clap_id` here.** This is the one deliberate departure from
vxn-1b, where the descriptor table *is* the CLAP param table. vxn-4 exposes
eleven CLAP params on purpose ([params.rs:1-18](../../vxn-4/crates/vxn4-clap/src/params.rs#L1-L18))
and this ticket does not reopen that. The descriptor table is a **UI and preset**
table, and the mapping between the two id spaces is explicit and one-way.

**Correction to an earlier draft of this ticket:** the eleven host params are
*not* patch fields that happen to be CLAP-exposed. Which patch is loaded, the
oversampling quality and the eight macro knob positions are performance and
project state — a macro's position belongs to the song, not to the sound — so
they sit in a second region of the table that the preset codec does not write.
The one that looks like an exception is gain, and it is two different params:
`Patch::gain` is the patch's own measured loudness trim and *is* patch state,
while the CLAP `MasterGain` is the player's output trim and is not. Both exist
in the table, named `gain` and `master-gain`.

## Acceptance criteria

- [ ] `vxn4-engine::params` exposes a `ParamDesc` table covering every field of
      `Patch`: per-operator `OpConfig` fields, the per-operator `EgParams`, the
      64 PM route constants, the 8 sum-bus sends, the 48 matrix slot depths,
      and the patch's own gain trim.
- [ ] The table additionally carries the eleven **host** params in a second
      region, so the faceplate can bind them by name like everything else, with
      `is_patch_field` as the predicate the preset codec filters on.
- [ ] Names embed the index and are stable: `op3_damp_hz`, `op3_eg_t1`,
      `pm_3_5_const`, `out_3_const`. Documented scheme, one function that
      builds a name from its indices and one that parses it back.
- [ ] Every descriptor carries: machine name, display label, range, taper,
      unit/formatting, and the default that the current factory patches imply.
- [ ] `ParamId` is a distinct type from the CLAP id, with an explicit
      `clap_id_for(ParamId) -> Option<usize>` and its inverse. No `as` casts
      between the two anywhere.
- [ ] Enum-valued fields (waveform) store and parse a kebab machine name, with
      an unknown name decoding to the default rather than failing.
- [ ] Round-trip property test: for a random `ParamId`, `parse(name(id)) == id`,
      over the whole table.
- [ ] A test asserts the table's length equals the derived count
      (`NOPS * N_PER_OP + NOPS*NOPS + NOPS + N_MATRIX_SLOTS + 1 + host`), so
      adding a field to `OpConfig` without adding a descriptor fails the
      build's tests.
- [ ] A test asserts every taper is invertible over its range — a
      `BipolarExp` whose `mid` is not strictly below `max/2` silently degrades
      to linear and an `Exp` pinned outside its range emits NaN into a fader,
      and both are configuration errors in this file rather than bugs in the
      shared taper math.

## Notes

Scale: [`NOPS`](../../vxn-4/crates/vxn4-dsp/src/ops.rs#L111) is 8, so the PM
block alone is 64 entries and the table is around 200. It should be **generated
from the index arithmetic**, not written out — the same way
[`matrix.rs`](../../vxn-4/crates/vxn4-engine/src/matrix.rs) already derives
`pm_dest_index` / `out_dest_index` / `damp_dest_index` rather than listing them.
Reuse those helpers so the two orderings cannot drift.

Defaults come from the factory patches, but not all seven agree. The default in
the descriptor is the value a *fresh* patch takes, which for most fields is
`OpConfig::default()` ([ops.rs:191](../../vxn-4/crates/vxn4-dsp/src/ops.rs#L191))
and `EgParams::default()` ([eg.rs:52](../../vxn-4/crates/vxn4-engine/src/eg.rs#L52)).
The factory patches then serialise as deviations from that, which is a useful
early check on whether the defaults are the right ones.

`vxn_core_app::ParamDesc` already exists and is what vxn-1b's codec and web glue
both consume. Prefer reusing it outright; if vxn-4 needs a field it does not
carry, widen the shared type rather than defining a parallel one, and say so in
the close-out.

Deliberately **not** in scope: any field the mockup draws that the engine does
not have (filter, LFOs, ADHSR mod sources, FX, key scaling, rational tuning).
Those get descriptors when they get implementations. The table describes what
exists.

## Close-out (2026-09-14)

- `vxn4-engine::params` carries 252 descriptors:
  [params.rs](../../vxn-4/crates/vxn4-engine/src/params.rs). The patch region is
  241 (8 ops × 15 fields, 64 PM constants, 8 sum-bus sends, 48 matrix slot
  depths, patch gain); the host region is the eleven CLAP params.
  `the_table_is_the_size_the_layout_implies` pins the arithmetic, so a field
  added to `OpConfig` without a descriptor fails the build's tests.
- Names are kebab, zero-based, index embedded, matching `matrix.rs`'s
  `DEST_NAMES` — built with `concat!` rather than formatted, which is what lets
  `ParamDesc` stay a plain static. `every_id_round_trips_through_its_name` and
  `names_are_unique` cover the scheme; `pm_ids_agree_with_the_matrix_layout`
  asserts a PM param and its destination name the same route.
- `ParamId` is the model id and `clap_id` the host id, joined only by
  `clap_id_for` / `param_for_clap`. `the_two_id_spaces_meet_only_at_the_host_region`
  holds the mapping; a grep for casts between the two returns nothing.
- `variant_or_default` makes an unknown enum label take the descriptor default
  rather than fail the file — `an_unknown_enum_label_falls_back_to_the_default`.
  0383 surfaces the substitution as a warning.
- `every_taper_is_invertible` round-trips every descriptor's taper over 21
  fader positions. It is the one that catches configuration errors in this file
  (a `BipolarExp` with `mid >= max/2` degrades silently to linear; a mispinned
  `Exp` emits NaN into a fader) rather than bugs in the shared taper math.
- Two corrections to the ticket as written, both recorded in it above: matrix
  slot depths were missing from the criteria, and the host params are a separate
  region rather than patch fields that happen to be CLAP-exposed —
  `Patch::gain` (`gain`) and the player's output trim (`master-gain`) are
  different params and both exist.
- 27 tests under `params::tests`; `cargo test -p vxn4-engine` green.
