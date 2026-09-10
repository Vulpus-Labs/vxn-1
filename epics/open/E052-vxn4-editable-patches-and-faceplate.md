---
id: E052
product: vxn-4
title: "vxn-4 editable patches — name-keyed TOML presets, and the faceplate in the plugin"
status: open
created: 2026-09-10
---

> Two halves that need each other. Patches stop being hardwired Rust and become
> **named, addressable, serialisable state owned by the main thread**; and the
> HTML faceplate that has been prototyped in
> [`vxn-4/ui-mockup/index.html`](../../vxn-4/ui-mockup/index.html) becomes the
> plugin's real editor, wired to what the engine has and visibly greying what it
> does not.

## Goal

Today a vxn-4 patch is a `Patch` struct built by a `const`-ish factory function
and **owned by the audio thread** — [`Engine::patch`](../../vxn-4/crates/vxn4-engine/src/engine.rs#L273),
swapped wholesale by [`set_patch`](../../vxn-4/crates/vxn4-engine/src/engine.rs#L449).
There are seven of them, they cannot be edited, and the `clap.state` blob is
eleven floats because eleven floats is genuinely all the user state there is
([state.rs:20-24](../../vxn-4/crates/vxn4-clap/src/state.rs#L20-L24) says so, and
says this is the moment that changes).

When this epic closes:

- Every field of a patch has a **stable name**, a descriptor (range, taper,
  formatting), and a default — so a patch can be written as sparse TOML that
  auto-adopts improved defaults, and so the faceplate can bind by name rather
  than by position.
- The **main thread owns the patch.** The audio thread reads it and never owns
  it: values cross as atomics, topology crosses as an SPSC ring, bulk changes
  cross as one snapshot. Truth flows main → audio and never back.
- Presets load, save and restore — from the baked factory bank, from user files
  on disk, and across a host project save.
- The faceplate opens in the CLAP plugin, driving the controls the engine
  actually has, with everything else present but greyed rather than absent.

## Why now

1. **The two halves share one prerequisite.** Serialising a patch and binding a
   faceplate to it are the same problem twice: both need every patch field to
   have a name, a range and a default. Doing either alone means building the
   descriptor table anyway and then building the other half on top of it.
2. **The ownership inversion gets more expensive with every patch feature.**
   `Engine` owning `Patch` is load-bearing in the render path today. It is a
   contained change while the only writer is `set_patch`; it stops being
   contained the moment an editor is writing individual fields at pointer rate.
3. **The design work is done.** vxn-1b already solved this exact problem and the
   answers are in the tree, not in anyone's head: sparse name-keyed TOML
   ([preset.rs](../../vxn-1b/crates/vxn1b-engine/src/preset.rs)), atomics for
   values with a lock the audio thread never takes for the rest
   ([shared.rs](../../vxn-1b/crates/vxn1b-engine/src/shared.rs)), and an SPSC
   topology ring with `Edit` per field and `Snapshot` for bulk
   ([topology.rs](../../vxn-1b/crates/vxn1b-engine/src/topology.rs), ADR 0003 §4).
   The shared scaffold for the file envelope is already factored out into
   [`vxn-preset`](../../crates/vxn-preset/src/lib.rs).
4. **The faceplate is designed and reviewed.** The mockup has been through
   several rounds and settles the layout, the control vocabulary and the
   interaction rules. Porting a settled design is a different job from
   designing one in Rust.

## Design decisions

**Names, not positions.** A patch serialises as sparse TOML keyed by descriptor
name, with the array index in the name (`op3_damp_hz`, `pm_3_5_const`) — vxn-2's
scheme for its own array-heavy operator params. Only fields that deviate from
their default are written. A positional format would rot the first time the
field order changed; a name-keyed one survives reordering and auto-adopts
improved defaults.

**`ParamId` decouples from `clap_id`.** This is the one place vxn-4 must depart
from vxn-1b, where the descriptor table *is* the CLAP param table. vxn-4
deliberately exposes eleven CLAP params and no more
([params.rs:1-18](../../vxn-4/crates/vxn4-clap/src/params.rs#L1-L18)) — the
argument there is unchanged and this epic does not reopen it. So the descriptor
table is a **UI and preset** table of which eleven entries happen to be
CLAP-exposed, and the two id spaces are separate types.

**Two channels to the audio thread, not one.** Values are latest-wins and
coalesce a knob drag for free, so they ride atomics. Topology — waveform
selections, route enables, matrix sources and curves — is neither a CLAP param
nor rare enough to justify a lock, so it rides the ring. Bulk changes (preset
load, `state.load`) ride the ring as one snapshot, which is also the overflow
backstop.

**The faceplate is honest about the gap.** The mockup draws a filter, two LFOs,
two ADHSR mod sources, key scaling, rational tuning, eleven waveforms and an FX
chain. The engine has none of those. They ship greyed, not hidden — a greyed
control says "planned, not yet"; an absent one says "never".

## Scope

**In:**

- A `vxn4-engine::params` descriptor table covering every patch field.
- Ownership inversion: `SharedParams` (atomics + main-thread-authoritative
  tables + reload flag) and an SPSC topology ring; `Engine` reads, never owns.
- Sparse TOML codec over [`vxn-preset`](../../crates/vxn-preset/src/lib.rs).
- `clap.state` v2 — the header is already shaped for a payload.
- User-preset filesystem IO and the `vxn_core_app::PresetStore` adapter.
- A `vxn4-app` crate: `ParamModel` impl plus controller wiring.
- A `vxn4-ui-web` crate: the mockup's assets, the wry host, and the CLAP `gui`
  extension in `vxn4-clap`.
- Binding the live controls; greying the unbuilt ones.
- Preset bar and browser on the faceplate.

**Out (deferred, and each its own future ticket):**

- **Building any of the greyed features.** The filter, the two LFOs, the two
  ADHSR mod sources, the FX chain, per-op key scaling, rational `num/den`
  tuning, the seven additional waveforms and the new matrix sources are all
  engine work this epic deliberately does not do. It makes room for them.
- Widening the CLAP param table beyond the eleven.
- A patch **browser corpus** beyond one level of user folders (vxn-1b's shape).
- Preset categories, tagging, or search beyond substring.
- The web/wasm build of the vxn-4 faceplate.

## Planned tickets

- [ ] 0381 — Patch descriptor table: every field named, ranged, defaulted.
- [ ] 0382 — Invert patch ownership: shared store + topology ring.
- [ ] 0383 — Sparse TOML preset codec over `vxn-preset`.
- [ ] 0384 — `clap.state` v2 carries the patch payload.
- [ ] 0385 — User-preset filesystem IO + `PresetStore` adapter.
- [ ] 0386 — `vxn4-app`: `ParamModel` impl and controller wiring.
- [ ] 0387 — `vxn4-ui-web` + the CLAP `gui` extension: the faceplate opens.
- [ ] 0388 — Bind the live controls; grey the unbuilt ones.
- [ ] 0389 — Preset bar and browser on the faceplate.

0381 gates everything. 0382 and 0383 are independent of each other and can run in
parallel once 0381 lands. 0387 is mostly an asset port and can start immediately,
in parallel with the whole engine half — it only needs 0386 to show real values.
0384 needs both 0382 and 0383. 0389 needs 0385 and 0388.

## Risks

- **The ownership inversion is the load-bearing step**, and its failure mode is
  a priority inversion rather than a test failure: an audio thread that takes a
  lock the editor holds drops out under load and passes every test on an idle
  machine. Mitigated by copying vxn-1b's split exactly — the audio thread must
  not take the topology mutex at all, which is a structural property, not a
  timing one, and can be asserted by construction.
- **A wide descriptor table is a wide surface to get wrong once.** 64 PM routes
  and 8 operators means the table is mostly generated, and a generated table
  with an off-by-one in its name scheme produces presets that load into the
  wrong field silently. Needs a round-trip property test over random patches,
  not example tests.
- **Silent misparse is worse than refusal.** A patch that loads with three
  fields quietly at their defaults is a support burden that looks like a synth
  bug. Unknown keys must be collected as warnings and surfaced, per vxn-1b.
- **The greyed set is large enough to look broken.** More of the faceplate is
  inert than live at the end of this epic. Worth deciding how a greyed control
  presents before 0388 rather than during it.
- **Editor scope creep**, the same risk E050 carried and hit. 0387–0389 are
  three tickets of UI work against a design that is settled — the mitigation is
  that the mockup is the spec, and layout questions get answered by editing the
  mockup first.

## Acceptance

- Every field of every factory patch round-trips through TOML: write, read,
  compare, byte-identical rendered audio.
- A patch saved by a build with fewer fields loads into a build with more, and
  the missing fields take their descriptor defaults.
- The audio thread never takes a lock and never allocates on any patch-edit
  path, including a preset load landing mid-render.
- A host project save and reload restores the full patch, not just eleven
  floats, and a stale v1 blob still loads to a known factory patch.
- The faceplate opens in a CLAP host, and every control that drives something
  drives it; every control that does not is visibly greyed.
- Moving a live control on the faceplate is audible, survives a host save, and
  writes into a user preset that reloads identically.
