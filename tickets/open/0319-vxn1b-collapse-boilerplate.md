---
id: "0319"
product: vxn-1b
title: "Collapse the hand-copied boilerplate: FX run_* x5, smoother pattern x4, matrix enum tables x4"
priority: low
created: 2026-08-26
epic: E047
depends: []
---

## Summary

Nine clusters of copy-paste, none urgent, all with the same cost: adding the
next member of the set means editing N places, and nothing fails if you edit
N-1.

### Where a macro is required, not optional (hot path)

**1. Five byte-identical FX `run_*` methods** —
[fx.rs:316-360](../../vxn-1b/crates/vxn1b-engine/src/fx.rs#L316-L360).
`run_chorus` / `run_phaser` / `run_delay` / `run_reverb` / `run_dynamics` differ
only in the slot constant and the kernel field; each is
`if !on[S] && fades[S].current() == 0.0 { return }; let (wl, wr) = kernel.process(xl, xr); blend(...)`.

These are **per-sample hot**, so a trait-object loop would be a deoptimisation —
`macro_rules! fx_slot!(run_chorus, CHORUS, chorus)` generates identical code and
is the right answer. See [[vxn1-fx-dual-chain-internally]] for why the mono path
has no upside here.

While in the file: [fx.rs:305 `clear_slot`](../../vxn-1b/crates/vxn1b-engine/src/fx.rs#L305)
ends in `_ => self.dynamics.clear()`, so a bogus slot index silently clears the
compressor. Make that arm `DYNAMICS =>` plus an explicit `_ => {}`. And
[fx.rs:257-275](../../vxn-1b/crates/vxn1b-engine/src/fx.rs#L257-L275) has
`let o = self.run_x(xl, xr); (xl, xr) = o;` five times where destructuring
assignment works directly.

**2. The four-step smoother pattern, written out four times** —
[bank.rs:896-899, 977-995, 1007-1010, 1131-1152](../../vxn-1b/crates/vxn1b-engine/src/bank.rs#L896).
For each of pitch/pwm/xmod/pan: declare `[bool; N]`, set
`x_active[v] = active[v] && smooth.x_active(v, tgt[v].x)`, reduce with `.any()`,
tick under the flag. `mod_smoothing.rs` mirrors it with a quadruplicated
`X_active` / `tick_X` / `X_current` / `snap_X` API
([:244-350](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs#L244-L350) —
**12 near-identical delegating wrappers**).

The irony is on the record: `LaneOnePole`'s own doc
([mod_smoothing.rs:82](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs#L82))
states the design goal as *"keeps a new smoothed dest to one field instead of a
field plus four hand-written methods"* — and the file then writes three-to-four
hand-written methods per quantity. A fifth smoothed dest currently means edits in
eight places.

`tick_pitch` ([:275-285](../../vxn-1b/crates/vxn1b-engine/src/mod_smoothing.rs#L275-L285))
is the exception: its unrolled two-stage cascade with hoisted coeff is
per-quantum hot and justified by the C1-continuity argument in the module docs.
**Do not loop it.**

### Where it is just repetition

**3. Four parallel tables per matrix enum** —
[matrix.rs:96-108, 239-258, 351-359](../../vxn-1b/crates/vxn1b-engine/src/matrix.rs#L96).
For `SourceId`: the variant list, `from_u8`, `SOURCE_NAMES`, `SOURCE_LABELS`,
`is_bipolar` — five independently-written lists keyed on one discriminant. Same
for `DestId` (five) and `Curve` (four). The tests check only *lengths*
(`name_and_label_tables_are_sized`) and round-trip, **not that name N describes
variant N** — a transposed label pair would ship silently. One `matrix_enum!`
macro taking `Variant = n, "wire-name", "Label", polarity` rows emits all five.

**4. Four hand-rolled `COUNT` + `from_index` impls beside a macro that does it** —
`StackWidth` / `VoiceMode` / `StackDistrib` / `CrossModType` at
[params.rs:54-152](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L54-L152).
The file already defines
[`indexed_param_enum!`](../../vxn-1b/crates/vxn1b-engine/src/params.rs#L157)
which generates exactly `COUNT`/`index`/`from_index`/`all` safely and uses it
for `ParamId` — but not for these four, whose bodies are the error-prone
hand-written form (`CrossModType::COUNT = 4` is a bare literal, unlike its three
siblings). Extend the macro with a `default` marker and apply it.

**5. `codec.rs`'s four hand-maintained 16-arm matches** — `Event::tag()` and
`Event::offset()` ([:196-244](../../vxn-1b/crates/vxn1b-wasm/src/codec.rs#L196-L244)),
plus encode and decode. [[0312]] deletes the encode arm; hoisting `offset: u8`
into a `Slot { offset, kind }` kills `offset()` entirely, and `#[repr(u8)]`
discriminants can derive `tag()`.

**6. `WebPresetStore`'s `.lock().map_err(|_| "user store poisoned")` ×8** —
[web-controller/src/lib.rs:164-225](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L164-L225).
Nine of eleven trait methods are the same three-line incantation. One
`fn user(&self) -> Result<MutexGuard<'_, UserState>, String>` and each becomes
`self.user()?.load(path)`.

**7. Three byte-identical `*_buf_reserve` functions** —
[:945](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L945),
[:1117](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L1117),
[:1148](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L1148) — differing
only in which `Vec<u8>` they touch.

**8. The two-argument staging decode ×5** —
[:955-1040](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L955-L1040).
`save_preset` / `rename_preset` / `move_preset` / `rename_folder` /
`hydrate_preset` each hand-roll `arg_string(0, a)` + `arg_string(a, b)`, and two
of them repeat the identical `ARG_NONE → None` block. One
`fn arg_pair(&self, a: u32, b: u32) -> (String, Option<String>)`.

**9. Duplicated drag wiring across the widget factories** —
[fader.js:404-433](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/fader.js#L404-L433)
vs [:495-527](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/fader.js#L495-L527).
`makeDial` and `makeBipolar` carry equivalent `writeFromDrag` clamps, the same
`attachValuePop` forward-declaration dance, the same `wireDrag` call with
`{raf:true, downContext:...}`, and identical gesture brackets — differing only in
axis, sign, and `DIAL_RANGE_PX`(200) vs `RANGE_PX`(400). `makeFader:82-100`
repeats the shim a third time; `makeWave:260-280` a fourth, with the forward
declaration inverted. One `wireNormDrag(el, id, { axis, rangePx, paint, getNorm })`
in `util/drag.js` removes ~90 lines.

Also: [fader.js:326](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/fader.js#L326)
`DIAL_RANGE_PX = 200; // px for full 0..1 travel (matches the fader)` — it does
not match: the vertical fader is absolute-mapped over `--fader-h` by
`wireFaderDrag`, not delta-mapped. Fix the comment or the constant, and put
`makeBipolar`'s twin declaration beside it rather than inside the function.

### Smaller, same pass

- Five identical pointer accessors in
  [host.rs](../../vxn-1b/crates/vxn1b-wasm/src/host.rs#L152) — one
  `ptr_accessor!(name, field)` macro.
- `drain_meters` ([host.rs:300-317](../../vxn-1b/crates/vxn1b-wasm/src/host.rs#L300-L317))
  goes array → struct → array, re-flattening into a hand-maintained order that
  must match `MeterTap`'s discriminants. `drain_into(&mut h.meters)` is one line
  and order-correct by construction.
- `sync_partner_clap_id` / `rate_partner_clap_id`
  ([sync.rs:24,37](../../vxn-1b/crates/vxn1b-engine/src/sync.rs#L24)) are
  hand-mirrored inverses kept in step by hand — one
  `const SYNC_PAIRS: [(ParamId, ParamId); 3]` searched both ways.
- Four copies of `synths[0].x(); if layer2_on { synths[1].x() }`
  ([engine.rs:484-561](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L484-L561)).
- Three "lit toggle" CSS blocks with the same hardcoded hexes and no variables
  ([faceplate.css:318-336, 1066-1070, 1315-1329](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.css#L318)).
- `cutoffInteractionOverride` / `cutoffNormOverride` / `cutoffDisplayOverride`
  ([dispatch.js:352-368](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L352-L368))
  each open with the identical null guard.
- `descriptor_to_json` / `taper_to_json`
  ([ui-web:537,566](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L537)) duplicate
  the shared crate — `taper_to_json` byte-identically — with a doc that concedes
  it and plans for divergence (*"if the two ever diverge, reconcile here"*).
  Also `strip_esm_exports` ([:493](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L493)),
  a one-line alias whose doc admits it, and two no-op config assignments at
  [:78-79](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L78-L79) re-setting the
  exact defaults `WebEditorConfig::new` already applies.

## Acceptance criteria

- [ ] Adding a sixth FX slot, a fifth smoothed dest, a new matrix source or a
      new value-enum variant each requires editing exactly one place.
- [ ] The matrix enum tables are generated, and a test would fail if name N did
      not describe variant N — the current length-only check is not enough.
- [ ] `clear_slot`'s catch-all no longer silently clears the compressor.
- [ ] `tick_pitch`'s unrolled cascade is untouched.
- [ ] `busy_profile` / `route_profile` unchanged within noise — the FX macro in
      particular must generate identical code, so check the asm dump if the
      numbers move at all ([[vxn1-neon-grep-pitfall]] on how to read it).
- [ ] Full suite green under [[0321]].

## Notes

- Lowest priority in [[E047]] and the most deferrable. Nothing here is a defect;
  it is all "the next person pays".
- The matrix-enum item is the exception in terms of risk: a transposed label pair
  is invisible until a user reads a wrong name in the mod matrix, and the current
  tests cannot catch it. If only one item from this ticket gets done, do that one.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. One `cargo test` at a time —
  [[vxn-no-parallel-cargo-test]].
