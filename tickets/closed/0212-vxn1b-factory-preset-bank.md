---
id: "0212"
product: vxn-1b
title: "Factory preset bank — matrix-idiom init set incl. wheel-vibrato + MPE-pressure demos"
priority: medium
created: 2026-07-29
epic: E038
depends: ["0209", "0210", "0211"]
---

## Summary

Ship a small **factory preset bank** for VXN1b, embedded via `include_dir`. Tune
the set to the matrix routing idiom, and include two demos that showcase the
variant's flexibility: a **wheel-gated vibrato** (scale-source) and an
**MPE-pressure** patch (aftertouch → cutoff/amp). [[E038]].

## Design

**Format.** Name-keyed sparse TOML (ticket 0203,
[preset.rs](../../vxn-1b/crates/vxn1b-engine/src/preset.rs)): only non-default
params written; matrix as `[[matrix]]` array (source/dest/curve/scale-src by
kebab name; slots with `source: none`/`dest: none` omitted).

**Embed.** Add an `include_dir` factory bank (VXN1b has none today — factory is
in-code `PluginState::factory_default()` in
[state.rs](../../vxn-1b/crates/vxn1b-engine/src/state.rs)). Wire a `factory.rs`
enumerating the bundled TOMLs. **Touch `factory.rs` before install** —
`include_dir!` emits no rerun-if-changed (`vxn2-include-dir-no-rerun`).

**Demos:**

- **Wheel-gated vibrato** — LFO1 → Pitch depth, with mod-wheel as **scale-source**
  on that slot, so vibrato only sounds when the wheel is up.
- **MPE-pressure** — channel/poly pressure (aftertouch) → cutoff and/or amp via
  matrix slots, exercising the E036 MPE source.

**Legality.** All presets original subtractive patches — no DX7 rips, no legal
posture concern (contrast `vxn2-factory-preset-legal-posture`).

## Acceptance

- A factory bank of original presets embeds via `include_dir` and loads in the
  browser.
- Includes the wheel-gated-vibrato and MPE-pressure demos, both audibly doing
  what they claim.
- Each preset round-trips through save → reload (sparse TOML) with no drift.
- `factory.rs` touched in the install path so bank edits actually recompile.

## Close-out (2026-08-14)

### Scope taken from the epic, not this ticket

The ticket predates E039 and asks for the two single-layer demos only. [[E039]]'s
entry for 0212 is the current spec — *"matrix idiom **+ two layers** … plus
**split** and **dual** demos exercising both synths + LFO2 sync"* on the [[0221]]
format. Built to that.

### Bank

Eight presets, all original subtractive patches (no legal-posture concern —
contrast [[vxn2-factory-preset-legal-posture]]), under
`crates/vxn1b-engine/presets/factory/<Category>/<name>.toml`:

| Category | Preset | What it demonstrates |
|---|---|---|
| Bass | Ladder Bass | The plain matrix idiom: Env 1 → cutoff as a *route*, not a panel depth |
| Bass | Wide Sub | `stack_width` 4 as a voicing (0266) — an 8-note pool of fat unison |
| Lead | **Wheel Vibrato Lead** | **Ticket demo.** LFO 1 → Pitch scaled by the mod wheel |
| Lead | Unlocked PWM | Per-osc PWM dests (0261): two LFOs at different rates, widths unlocked |
| Pad | **Pressure Pad** | **Ticket demo.** Aftertouch → cutoff + amp (E036 MPE source) |
| Pad | Drifting Pad | LFO 2 → Pan auto-pan, only expressible since 0260 |
| Split | **Split Bass and Lead** | **Epic demo.** Bass below C3, lead above; two private matrices |
| Dual | **Dual Locked Sweep** | **Epic demo.** Panned/detuned apart with `lfo2-link` phase-locking both |

### One trap worth recording

The presets were **generated** rather than hand-written: a throwaway
`examples/gen_factory.rs` built each as a real `PluginState` and wrote it through
`write_preset`, so every file is sparse, correctly named and parses by
construction. The generator is not kept — the TOMLs are the artifact and the
tests below are the contract.

Building them that way surfaced the trap. The obvious base for a preset is
`default_patch()`, but that seeds **slot 1 with `LFO1 → Pitch @
DEFAULT_VIBRATO_DEPTH`** (0.16) to reproduce VXN1's always-on vibrato. Right for
an init patch; wrong for a designed one — and it would have sat an *ungated*
vibrato route beside the gated one in the wheel demo, silently defeating the
thing the demo exists to show. The bank therefore seeds only what every patch
needs (`Env2 → Amp`, or there is no VCA; `Spread → Pan`, or the Spread knob is
inert) and routes slot 1 explicitly. `factory::tests` asserts the wheel demo has
exactly **one** route into Pitch, so this cannot regress.

### Embedding + store

- [factory.rs](../../vxn-1b/crates/vxn1b-engine/src/factory.rs) mirrors vxn-2's
  bank: `include_dir!` over the tree, directory = category, `[meta] name` =
  display name. `include_dir = "0.7"` added to the crate.
- `EnginePresetStore`'s three factory methods are wired
  ([preset_io.rs](../../vxn-1b/crates/vxn1b-engine/src/preset_io.rs)), replacing
  the `0` / `"no factory bank yet"` stubs. Stale module docs saying the bank is
  empty are corrected.
- **`xtask` touches `factory.rs` before every bundle** (`touch_factory`) —
  `include_dir!` emits no `rerun-if-changed`, so without it a bank edit silently
  doesn't ship ([[vxn2-include-dir-no-rerun]]).

### Tests

`factory::tests` (10): non-empty; every file parses with **zero** warnings;
**every preset round-trips** write → read with no param, topology or keyboard
drift; no half-wired slot (source set, dest `none` — the least visible way to
break a preset); every preset drives `Amp`; the two named demos route what they
claim (the wheel slot really is `scale_src = mod-wheel`, and there is no second
ungated pitch route); the split and dual demos really enable Layer 2.

`preset_io::tests` (2): the bank loads **through `PresetStore`**, which is the
seam the browser actually uses — every index enumerates, exposes meta, and
decodes to a blob `restore_from_bytes` accepts. Past-the-end is an error, not a
panic or a silent factory-default.

290 Rust / 244 JS, 0 failures. vxn-1 unaffected (209 passed).

### Not done

- **Manual DAW check** ([[verify-audio-in-reaper]]): that the bank shows in the
  browser and each preset sounds as described — particularly that the wheel demo
  is *silent* until the wheel moves, and that the split lands at C3. Folds into
  0213's smoke.
