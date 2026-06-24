---
id: "0146"
product: vxn-2
title: "Dynamics params — CLAP table append, blob v15, decode, preset round-trip"
priority: medium
created: 2026-06-24
epic: E028
depends: ["0145"]
---

## Summary

Second ticket of [E028](../../epics/open/E028-vxn2-fx-dynamics-block.md).
Append eight `dyn-*` params at the end of the flat CLAP table, bump
the blob version to v15, add `OFF_DYNAMICS`, `DynamicsParams` decode,
and wire preset round-trip. Append — not insert — keeps every
existing id (filter, phaser, limiter, hp-cutoff) stable so saved DAW
sessions and presets survive.

## Design

Files:

- `vxn-2/crates/vxn2-engine/src/shared.rs` — param table, offsets,
  decode, blob migration.
- `vxn-2/crates/vxn2-engine/src/params.rs` — CLAP param entries.
- `vxn-2/crates/vxn2-engine/src/default_patch.rs` — default values.
- `vxn-2/PARAMETERS.md` — doc entry (and backfill phaser if still
  missing — see Notes).

**Append-at-tail follows the E025 phaser precedent.** Mirror
`N_PHASER_PARAMS_V14` (`shared.rs:161-167`) with
`N_DYNAMICS_PARAMS_V15` (8), bump `BLOB_VERSION` to 15, add
`LEGACY_V14_PARAM_COUNT = TOTAL_PARAMS - N_DYNAMICS_PARAMS_V15`, and
extend the blob migration so a v≤14 blob decodes with the eight
trailing slots defaulted (i.e. dynamics off, identity params).

**New ids (appended at table tail, after the phaser block):**

| id name           | type | range          | default | notes                              |
|-------------------|------|----------------|---------|------------------------------------|
| `dyn-on`          | bool | 0/1            | 0       | gate; `0` = bit-exact passthrough  |
| `dyn-threshold`   | f    | −60..0 dB      | −12.0   |                                    |
| `dyn-ratio`       | f    | 1..20          | 4.0     |                                    |
| `dyn-attack`      | f    | 0.1..200 ms    | 10.0    |                                    |
| `dyn-release`     | f    | 5..1000 ms     | 100.0   |                                    |
| `dyn-makeup`      | f    | 0..24 dB       | 0.0     |                                    |
| `dyn-drive`       | f    | 0..36 dB       | 0.0     | `0` = identity (no harmonics)      |
| `dyn-mix`         | f    | 0..1           | 1.0     | dry/wet within the dynamics block  |

`OFF_DYNAMICS = (live tail before dynamics block)`. Decode arm mirrors
the phaser block (`shared.rs:1347-1355`):

```rust
self.dynamics = DynamicsParams {
    on: shared.get(pb + OFF_DYNAMICS) >= 0.5,
    threshold_db: shared.get(pb + OFF_DYNAMICS + 1),
    ratio:        shared.get(pb + OFF_DYNAMICS + 2),
    attack_ms:    shared.get(pb + OFF_DYNAMICS + 3),
    release_ms:   shared.get(pb + OFF_DYNAMICS + 4),
    makeup_db:    shared.get(pb + OFF_DYNAMICS + 5),
    drive_db:     shared.get(pb + OFF_DYNAMICS + 6),
    mix:          shared.get(pb + OFF_DYNAMICS + 7),
};
```

`EngineParams::dynamics: DynamicsParams` field, `Default::default()`
init, and a `set_params` call in `apply_block_params()` are wired in
0147 — this ticket only stops at decoding into `EngineParams`.

**Mod-matrix: NOT added.** Confirm by grep on completion. Dynamics is
host-automation only, same as phaser (E025 out-of-scope).

**Preset round-trip.** Per [[vxn2-preset-system]] presets are
name-keyed sparse TOML — new `dyn-*` keys default-fill on load, old
presets load unchanged with dynamics off. Add a round-trip test
covering an old preset (no `dyn-*` keys) and a new preset with all
eight keys set.

## Acceptance criteria

- [ ] Eight `dyn-*` ids appended at table tail; `TOTAL_PARAMS`
      updated; `BLOB_VERSION = 15`.
- [ ] `N_DYNAMICS_PARAMS_V15 = 8`, `LEGACY_V14_PARAM_COUNT` defined,
      blob migration for v≤14 → v15 fills the eight new slots with
      defaults (dynamics off → bit-identical render).
- [ ] `EngineParams::dynamics: DynamicsParams` populates correctly
      from the param table (decode arm in `shared.rs`).
- [ ] Default patch leaves `dyn-on = 0` (bit-identical to pre-epic).
- [ ] Round-trip test: a v14 blob loads with dynamics defaulted off.
- [ ] Round-trip test: a v15 preset round-trips all eight `dyn-*`
      keys byte-for-byte.
- [ ] `grep -i 'dyn\|dynamics' vxn-2/crates/vxn2-engine/src/matrix.rs`
      returns nothing.
- [ ] `PARAMETERS.md` has a `### Dynamics` subsection under
      `## Effects`.
- [ ] `cargo test -p vxn2-engine` passes.

## Notes

`PARAMETERS.md` does not currently list the phaser block — E025
shipped the params but the doc wasn't backfilled (see grep at the
bottom of E025 close-out vs. `vxn-2/PARAMETERS.md:262-285`). If the
gap is still there when this ticket runs, backfill phaser **and**
add dynamics in the same edit so the doc catches up.

Followed by 0147 (engine bus wiring), 0148 (faceplate).
