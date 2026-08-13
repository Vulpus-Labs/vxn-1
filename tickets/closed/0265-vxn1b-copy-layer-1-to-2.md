---
id: "0265"
product: vxn-1b
title: "\"Copy Layer 1 → Layer 2\" button — duplicate the patch, offset the detune"
priority: medium
created: 2026-08-08
epic: E039
depends: ["0263"]
---

## Summary

Building a fat doubled sound means recreating Layer 1's patch on Layer 2 knob by
knob. Add a one-shot **Copy Layer 1 → Layer 2** button: it duplicates Layer 1's
patch params and matrix topology onto Layer 2, leaves the mix controls alone, and
stamps a small oscillator detune offset on the copy so the two layers beat
against each other instead of summing.

The layers already decorrelate for free once they hold the same patch — each
`Synth` gets its own bank/LFO 2/drift/trim seeds
([synth.rs:37](../../vxn-1b/crates/vxn1b-engine/src/synth.rs#L37)), and
`VoiceTrim`'s test asserts the two layers never share a spread
([bank.rs:1077](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L1077)). So the
whole feature is a bulk store write plus a UI opcode.

One-shot copy, **not** a live mirror: a continuously-slaved Layer 2 would make
its 68 CLAP ids inert while linked, silently dropping any host automation written
against them, and can't be hidden from the host without a param rescan. That's a
separate design if it's ever wanted.

## Design

**Where the write lands.** The `SharedParams` store, on the main thread — the
same road `restore_from_bytes`
([shared.rs:259](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L259)) and
`edit_matrix_slot`
([shared.rs:202](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L202)) already
take. Add:

```rust
pub fn copy_layer(&self, from: Layer, to: Layer)
```

which, for each `PATCH_PARAMS` entry not in the exclusion list, does
`self.set(patch_clap_id(to, p), self.get(patch_clap_id(from, p)))`; copies the
matrix table `m[from] → m[to]` under the existing `lock()`; applies the detune
offset (below); then raises `reload`.

Echo to the host and the faceplate is then **free**: the audio thread's
`take_reload` re-syncs the engine from `engine_state()`
([clap/src/lib.rs:449](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L449)), the
per-block `local.publish` / `emit`
([clap/src/lib.rs:517-521](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L517-L521))
pushes the changed ids to the host, and the timer tick's `push_param_diffs` +
`push_matrix_echo` repaint the editor. No new echo path.

**Opcode.** The faceplate posts `copy_layer`. `parse_custom_ui`
([ui-web/src/lib.rs:83](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L83)) parses
it into a new payload type — a small `PatchOp::CopyLayer { from, to }` enum in
the engine, **not** a `KeyOp` variant, since `KeyOp` is defined as mutations of
`KeyState` and this touches params. Add the third downcast arm to `on_custom_ui`
([clap/src/lib.rs:399-406](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L399-L406)),
which already chains `KeyOp` then `MatrixEdit` the same way.

**Exclusions.** `LayerLevel`, `LayerMute` and `LayerDetune` are the mixer strip —
balance and placement between the two copies, not part of the sound — so they
stay untouched. `LayerPan` joins the list when
[0248](0248-vxn1b-layer-pan.md) lands.

**Detune offset — required, not a nicety.** `lane_phase` is a fixed function of
lane index with no seed
([bank.rs:135](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L135)) and both
allocators pick the same lane for the same note, so an exact copy with
`MasterDrift` at 0 renders *bit-identical* layers: +6 dB and no width at all.
After copying, set Layer 2's `layer_detune` ([0263](0263-vxn1b-layer-detune.md))
to `COPY_DETUNE_CENTS` — start at 6 ct — leaving Layer 1's alone, so the pair
sits a few cents apart out of the box.

`layer_detune` is the right knob rather than stamping `Osc1Fine` / `Osc2Fine`:
it moves the layer's whole pitch base (both oscillators and the sub together) and
leaves the per-osc fine tune saying what it's for. It also keeps the copy's one
sound-affecting edit visible in a single control the user can undo by eye.

This is what the `depends: ["0263"]` is for. If the button is wanted before 0263
lands, the fallback is stamping both `Fine` params at `layer1_value ± 6 ct`
(clamped to the ±50 ct range,
[params.rs:532](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L532)) and
swapping them for `layer_detune` afterwards — but prefer waiting.

**Layer 2 off.** Copying while in Single mode does nothing audible, so when the
current mode is Single the op also applies `KeyOp::SetKeyMode(1)` (Dual). If the
mode is already Split, leave it — the user chose that routing.

**Levels stay put.** Both `LayerLevel`s default to 1.0, so a copy is roughly
+6 dB. Deliberately not auto-trimmed: the balance is the user's, the detune takes
some of the coherence out of the sum, and a silent gain change on a button press
is worse than a loud one.

## Acceptance criteria

- [ ] Pressing the button makes every `PATCH_PARAMS` value equal across layers
      except `LayerLevel` / `LayerMute` and the two detuned `Fine` params. Test
      in `shared.rs` or an engine test walking the table by `patch_clap_id`.
- [ ] Layer 2's matrix topology (source / dest / curve / scale on all 16 slots)
      matches Layer 1's after the copy; depths follow as params.
- [ ] Layer 2's `layer_detune` reads `COPY_DETUNE_CENTS` after the copy while
      Layer 1's is untouched, and a rendered block from the two layers is **not**
      identical with `MasterDrift` at 0 — the null-doubling regression test.
- [ ] `LayerLevel` and `LayerMute` on Layer 2 survive the copy unchanged; so does
      Layer 1's `LayerDetune`.
- [ ] Copying from Single mode leaves the engine in Dual; copying from Split
      leaves it in Split.
- [ ] The faceplate repaints Layer 2's pane and the host sees the new values —
      manual check in Reaper (per [[verify-audio-in-reaper]]), no headless
      harness.
- [ ] `clap.state` round-trips after a copy with both layers intact.

## Notes

- Destructive: it overwrites whatever patch Layer 2 held, and 66-odd param
  changes land in the host's undo stack as one burst. Cheap safety is a confirm
  step in the UI (or a press-and-hold); decide when placing the control.
- Don't raise the per-param `gesture` flags around the bulk write — they exist to
  suppress host echo during a live drag ([shared.rs:165](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L165))
  and would only fight the repaint here.
- UI placement: the Layer pane, beside the key-mode / `lfo2_link` controls it
  shares an opcode hook with.
- LFO 2 is left unlinked — same rate, different seed, so the two copies drift
  slowly apart, which is usually what "fatter" wants. `lfo2_link` is there if not.
- Orthogonal to [0264](0264-vxn1b-32-lanes-unison-16.md): with 32 lanes per synth
  a copied pair is 32 + 32.

## Close-out (2026-08-13)

- **`SharedParams::copy_layer(from, to)`**
  ([shared.rs](../../vxn-1b/crates/vxn1b-engine/src/shared.rs)) walks
  `PATCH_PARAMS` by `patch_clap_id`, copies the matrix table under the existing
  `lock()`, stamps the detune offset, and raises `reload` — the same road
  `restore_from_bytes` and `edit_matrix_slot` already take. A self-copy returns
  early and does not flag a reload.
- **Exclusions** are `LayerLevel`, `LayerMute`, `LayerPan`, `LayerDetune`
  (`COPY_LAYER_EXCLUDED`). `LayerPan` was pencilled in as "joins the list when
  0248 lands" — it has, so it went in from the start.
- **`COPY_DETUNE_CENTS = 6.0`** stamped on the copy, source layer untouched. The
  ticket's `depends: ["0263"]` paid off: `layer_detune` existed, so the `Fine`-param
  fallback was not needed.
- **Opcode.** `PatchOp::CopyLayer { from, to }` in the engine
  ([engine.rs](../../vxn-1b/crates/vxn1b-engine/src/engine.rs)) — deliberately
  **not** a `KeyOp` variant, since `KeyOp` is defined as mutations of `KeyState`.
  `parse_custom_ui` gains a `copy_layer` arm and `on_custom_ui` a third downcast,
  chained after `KeyOp` and `MatrixEdit`.
- **Echo is free, as designed.** `local.publish(&StoreRef(…))`
  ([clap/src/lib.rs:556](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L556)) diffs
  the local mirror against the store each `process`, so main-thread writes reach
  the host on the next block; the timer tick's param diff + matrix echo repaint
  the editor. No new echo path, and **no gesture flags raised** — they suppress
  host echo mid-drag and would only fight the repaint.
- **Key mode.** Copying from Single lands in Dual; an existing Split is left
  alone.

### UI

A `Copy → L2` cell in the Voice panel strip
([faceplate.html](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html)),
hand-wired in `dispatch.js` like the LFO 2 link cell it shares an opcode hook
with. **Layer-1-only** via a new `[data-layer1-only]` rule mirroring the existing
`[data-layer2-only]` — the action reads *from* the edit layer, so it would be
backwards on the Layer 2 tab.

The ticket left the destructive-safety mechanism open ("a confirm step in the UI
(or a press-and-hold); decide when placing the control"). Chosen: **arm on first
press, copy on second**, with the cell reading `Sure?` while armed. It disarms on
a 2.5 s timeout *and* on any other pointer-down on the page, so an armed cell
cannot outlive the player's attention. Cheaper than a modal, and it cannot fire
from a stray tap.

### Tests

Engine (`shared::tests`):

- `copy_layer_duplicates_every_patch_param_but_the_mixer_strip` — walks the whole
  table by `patch_clap_id`, and asserts a pre-set Layer 2 mixer strip survives.
- `copy_layer_duplicates_the_matrix_topology` — all 16 slots, all four fields.
- `copy_layer_offsets_the_copys_detune_only`.
- `a_copied_pair_does_not_null_double` — the criterion's *rendered* check, not
  just a param assertion: drives a real `Engine` from the copied state with
  `MasterDrift` at 0 and compares each layer's contribution with the other muted.
  Verified to have teeth by zeroing `COPY_DETUNE_CENTS`, which fails it.
- `copy_layer_turns_on_layer_2_but_leaves_an_existing_split`.
- `copy_layer_raises_no_gesture_flags`.
- `copy_layer_onto_itself_is_a_no_op`.
- `clap_state_round_trips_after_a_copy` — both layers, params and topology.

View (`__tests__/copy-layer.test.js`): a single press arms rather than copying;
the confirming press sends `upper → lower` exactly once and disarms; timeout and
click-elsewhere both disarm.

281 Rust / 244 JS, 0 failures.

### Not done

- **Manual DAW check** ([[verify-audio-in-reaper]]): that the faceplate repaints
  Layer 2's pane and the host sees the ~66 new values. Worth folding into 0213's
  smoke, along with whether the 6 ct default reads as "fatter" by ear.
- `cargo fmt` deliberately not run — the crate carries pre-existing rustfmt
  diffs, so formatting would stomp unrelated work
  ([[vxn-concurrent-vxn2-work-no-git-add-all]]).
