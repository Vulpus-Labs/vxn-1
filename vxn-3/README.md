# VXN3

Synthesis drum machine — **no samples**. Every voice is generated. Designed to
make *interesting rhythmic patterns* accessible, and to play pitched hits as
readily as drum sounds. Third instrument in the Vulpus Labs line (after VXN1
subtractive polysynth, VXN2 6-op FM).

Status: **design** — ADR 0001 drafted, no crates yet.

## Premises

- **Sample-free.** All sound synthesised. Drum *and* pitched material from the
  same engine roster; "drum vs note" is mostly envelope + pitch-tracking, not a
  separate code path.
- **Pattern engine is the product.** A step sequencer (familiar) with
  high-leverage generative levers surfaced as few knobs: polymeter, retrig
  n-over-m, probabilistic / conditional trigs, per-step p-locks (step/ramp/latch
  — these *are* the automation mechanism), dub-style p-lockable FX sends.
- **Heterogeneous engines, per-track SoA.** Each track holds one active engine
  and its patch. Across tracks: scalar per-block dispatch (heterogeneous). Within
  a track: a 4-wide SoA state block that vectorises (NEON `f32x4`). What the
  lanes *mean* is the engine's choice — voices (poly) or modes (resonator).
- **Genre target v1: psychedelic / minimal techno.** Repetition + slow
  evolution + space + dub throws. Disciplines the feature set.

## Shared infrastructure

Reuses the line's CLAP shell (`clack`), HTML faceplate idiom, preset system,
and build/release pipeline (`vxn-core-*`, xtask). RT discipline carried over:
allocation-free process callback, no panics across the FFI boundary, permissive
licensing only (MIT / Apache-2.0).

## Design docs

- [ADR 0001 — overall design](adrs/0001-vxn3-overall-design.md)
- [ADR 0002 — FX architecture & routing](adrs/0002-vxn3-fx-architecture.md)

## Tickets

Per-project counter (see top-level `tickets/`). A vxn-3 ticket number always
refers to a ticket tagged `product: vxn-3`.
