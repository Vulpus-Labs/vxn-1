---
id: "0179"
product: vxn-3
title: "vxn-3 per-engine patch serialization — fill the reserved state bytes; deep patch round-trips through save/load + swap"
priority: medium
created: 2026-07-04
epic: E034
---

## Summary

Finish Phase 0 of the VXN3 roadmap: make each track's **deep per-engine patch**
(the synthesis params that live below the fixed host table — `amp_decay_s`,
`base_hz`, `noise_decay_s`, sweep/click/drive, modal Q profile, …) persist in
`clap.state`. Ticket 0174 froze a forward-compatible blob format with a reserved
`patch_len u16 = 0` slot per track precisely so this could land additively without a
format break. This ticket populates that slot: define a small, explicit per-engine
patch (de)serialization, wire it into save/load, and prove it round-trips **through**
an engine swap.

This is the last green square in the Phase-0 table (host params + state). It is the
foundation the voice-roster / kit epic (E034) builds on — a kit *is* a set of engine
patches, so kits cannot exist until a patch can be serialized. Land + test the
mechanism here against today's three engines so E034's new/enriched engines only have
to implement the trait, not debug the format.

Design ref: [ADR 0003](../../vxn-3/adrs/0003-vxn3-host-param-model.md) §Consequences;
[ADR 0005](../../vxn-3/adrs/0005-vxn3-voice-families-flavours-macros.md) (the deep
patch this serialises *is* a **flavour** — base vector + macro-binding table — not a
flat value list; get the layout right, the flavour store reads the same bytes);
[state.rs](../../vxn-3/crates/vxn3-clap/src/state.rs) module header (frozen format);
0174 close-out (reserved bytes contract).

## Design

- **Per-engine patch (de)serialization.** Add to the engine/voicing trait:
  `serialize_patch(&self, out: &mut Vec<u8>)` and
  `deserialize_patch(&mut self, bytes: &[u8]) -> Result<(), _>`. Keep it explicit
  field-by-field (LE), **not** `#[derive]`-on-everything, so the byte layout is
  reviewable and stable — same discipline as the outer blob. Each engine owns a tiny
  patch-version tag so a single engine's layout can evolve independently of the
  global format version.
- **Wire into [state.rs](../../vxn-3/crates/vxn3-clap/src/state.rs).**
  - `save`: replace the hardcoded `0u16` with the real `patch_len`; write the engine's
    patch bytes after the `kind` byte. Bump the global format version tag (v1 → v2).
  - `load`: read `patch_len`, take that many bytes, hand them to the rebuilt engine's
    `deserialize_patch` **after** the swap installs and **before** the macro/mix cache
    replay — deep patch is the base layer, host-table values (0171 cache) sit on top.
- **Restore order (tighten 0174's).** per track: rebuild engine from saved kind →
  `deserialize_patch` → replay macro/mix cache + p-lock invalidation. Document why
  patch precedes cache (host params are overrides on the patch, not the reverse).
- **Backward compatibility.** A v1 blob (`patch_len == 0`) must still load: the engine
  keeps its default patch, no error. New saves are v2. `resave` of a loaded v1 blob
  becomes v2 (documented; acceptable). Garbage / truncated / future-version still
  rejected as in 0174.
- **Round-trip through swap.** A project whose track was swapped to a different engine
  restores that engine *with its saved patch*, not the engine's default — extends the
  0174 swap test to assert patch values, not just kind.
- **Scope.** Sequencer/pattern state remains out of scope (as 0174) — note again where
  it lives so E034's kit work picks it up deliberately, not by accident.

## Acceptance criteria

- [ ] Engine trait exposes `serialize_patch` / `deserialize_patch`; all three current
      engines (Kick/Tone, Metal, Noise) implement them field-explicit + patch-version
      tagged.
- [ ] `clap.state` save writes real `patch_len` + patch bytes; load restores the deep
      patch to the rebuilt engine before macro/mix replay — a reload reproduces the
      audible patch, not just the mix.
- [ ] Round-trips **through** a swap: saved engine + patch restored, not default —
      integration tested with distinct non-default patch values.
- [ ] v1 blob (`patch_len == 0`) loads cleanly (default patch, no error); garbage /
      truncated / future-version still rejected. Format version bumped + documented in
      the [state.rs](../../vxn-3/crates/vxn3-clap/src/state.rs) header.
- [ ] `resave_is_byte_identical` holds for v2; a v1→load→save transition is covered by
      a test and documented as an intentional upgrade.
- [ ] `clap-validator` reports **0 failures** across the VXN3 sweep; `cargo test
      -p vxn3-clap -p vxn3-engine` green; alloc-trap tests still pass.

## Notes

- This closes Phase 0. On merge, the roadmap's Phase-0 table is fully green and E034
  (voice roster + kits) is unblocked.
- Get the per-engine patch layout right here — E034 kits persist the same bytes, and
  the roster's enriched/new engines (struck-resonator family, richer Kick/Noise) will
  each implement this trait. A sloppy format now is 17 modules of debt later.
- Belongs to the E034 voice-roster epic as its groundwork ticket; if E034 is not yet
  scaffolded when this is picked up, land it standalone — it stands on 0174 alone.

## Close-out (2026-07-04)

- **Trait.** `TrackEngine` gains `serialize_patch(&self, &mut Vec<u8>)` +
  `deserialize_patch(&mut self, &[u8]) -> Result<(),()>` with no-op defaults
  ([track_engine.rs:59](../../vxn-3/crates/vxn3-engine/src/track_engine.rs#L59)). All
  three engines implement them field-explicit LE behind a per-engine `PATCH_VERSION`
  byte: KickTone (4 f32s), Metal (5), Noise (4) —
  [kick_tone.rs](../../vxn-3/crates/vxn3-engine/src/engines/kick_tone.rs),
  [metal.rs](../../vxn-3/crates/vxn3-engine/src/engines/metal.rs),
  [noise.rs](../../vxn-3/crates/vxn3-engine/src/engines/noise.rs). Shared LE underrun
  reader in new [patch.rs](../../vxn-3/crates/vxn3-engine/src/patch.rs) (crate-internal).
- **Format v2.** [state.rs](../../vxn-3/crates/vxn3-clap/src/state.rs) `VERSION 1→2`;
  `save` writes real `patch_len` + engine patch bytes per track (module header + restore
  order documented); `load` surfaces per-track patch bytes via a `&mut [Vec<u8>; N_TRACKS]`
  out-param.
- **Restore order.** [lib.rs](../../vxn-3/crates/vxn3-clap/src/lib.rs) `PluginStateImpl::load`
  rebuilds each engine → `deserialize_patch` → sends over the swap ring, so the deep patch
  is the base layer and the macro/mix cache replays over it (host params override the patch,
  not the reverse). Survives inactive-load→activate via the shared swap ring.
- **Round-trip through swap** proven with distinct non-default values:
  `engines::tests::patch_round_trips_through_rebuild` (edit → serialize → fresh default
  engine → deserialize → identical audio, and ≠ default).
- **v1 compat / rejection.** `state::tests::v1_blob_loads_with_default_patch_then_upgrades`
  (patch_len==0 → default kept; resave upgrades to v2), `empty_and_garbage_rejected`,
  `future_version_rejected`, `engines::tests::patch_deserialize_tolerances` (empty ok,
  unknown-version tolerated, truncated rejected). `resave_is_byte_identical` holds for v2.
- **Gates.** `cargo test -p vxn3-engine -p vxn3-clap` green; `cargo clippy` on both crates
  0 warnings; alloc-trap tests (`three_engine_kit_is_allocation_free`, plocks) pass;
  `clap-validator validate vxn3.clap` — **0 failed** (state-reproducibility-basic/flush/
  buffered/null-cookies/invalid all PASSED; 5 note-ports tests skipped, land in 0186).
- Phase 0 closed. E034 (voice roster) unblocked: enriched/new engines implement the two
  trait methods; kits/flavours persist the same bytes.
