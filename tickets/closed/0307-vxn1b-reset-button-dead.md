---
id: "0307"
product: vxn-1b
title: "Keys panel Reset button is dead — reset_layer has no handler"
priority: medium
created: 2026-08-25
epic: null
depends: []
---

## Summary

The Keys panel's **Reset** button does nothing in the shipped VXN1b plugin.

[keys.js:131-142](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/keys.js#L131-L142)
wires `resetBtn` to post `reset_layer` (both layers in Whole, otherwise the edit
layer). Nothing consumes it:
[`parse_custom_op`](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L99) handles
`set_key_mode`, `set_split_point`, `set_lfo2_link`, `set_matrix`, `copy_layer`
and `set_scope_source` — no `reset_layer` — and a grep for `reset_layer` /
`ResetLayer` across all of `vxn-1b/**/*.rs` returns nothing. The opcode falls
through `parse_custom_op`'s `_ => None` and is dropped silently.

Inherited from the vxn-1 fork (E038), where it *is* live:
[vxn-app/src/controller.rs](../../vxn-1/crates/vxn-app/src/controller.rs)
implements `Vxn1UiCustom::ResetLayer` via `reset_layer`, bracketing each write in
a gesture so the host records one edit.

Found while routing the page's opcodes for
[0291](0291-vxn1b-faceplate-rewire.md). Not fixed there: 0291 is the web bridge,
and this is a native-side gap that the web port would otherwise faithfully
reproduce.

## Two more fork artifacts in the same file

Both cosmetic, both worth settling in the same pass:

- `set_edit_layer` ([dispatch.js:541](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L541),
  [keys.js:104](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/keys.js#L104)) is
  also unhandled Rust-side, but harmlessly: the page rebinds the edit layer
  locally and nothing needs the echo. vxn-1 emits `edit_layer_changed`; VXN1b
  never does, yet [dispatch.js:960](../../vxn-1b/crates/vxn1b-ui-web/assets/dispatch.js#L960)
  still has a handler for it.
- [keys.js:122-123](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/keys.js#L122-L123)
  says the readout will be corrected by "the echo from `split_point_changed`".
  VXN1b has no such event — split point arrives inside its `keys` record.

## Design

Decide which way it goes; both are defensible and the choice is a product call,
not a technical one:

- **Make it live.** Add a `PatchOp::ResetLayer` (it is a `SharedParams`
  operation like `CopyLayer` — it rewrites patch params, not `KeyState`),
  handle it in `parse_custom_op` and in `vxn1b-clap`'s custom-op chain, and
  respect the same `COPY_LAYER_EXCLUDED` question: does Reset also leave the
  mixer strip alone, or is a full patch reset the point? `copy_layer` excludes
  it deliberately ([shared.rs:44-48](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L44-L48)).
- **Remove the button.** Delete `resetBtn` and its handler from `keys.js` plus
  the markup, and drop the dead `edit_layer_changed` arm.

Making it live is the better default — a reset-to-default is genuinely useful on
a two-layer synth, and the button is already laid out and styled.

## Acceptance criteria

- [ ] The Reset button either resets the intended layer(s) or is gone; no
      silently-dropped opcode remains.
- [ ] If made live: a test asserts every patch param of the target layer returns
      to its descriptor default, that the *other* layer is untouched in Dual /
      Split, and that both are reset in Whole (the behaviour `keys.js` already
      describes).
- [ ] If made live: whether the mixer strip resets is decided and recorded, with
      the reason, alongside `copy_layer`'s exclusion.
- [ ] `edit_layer_changed`'s dead handler and the stale `split_point_changed`
      comment are resolved either way.
- [ ] A test (or a grep-sweep assertion) pins that every opcode the page can
      post has a handler — this class of fork artifact should not be able to
      recur silently.
- [ ] `cargo test -p vxn1b-ui-web` + the asset suite green.

## Notes

- The last criterion is the one with lasting value: three dead opcodes survived a
  fork because nothing checks that the sender surface and the parser agree.
- Whatever is decided, [0291](0291-vxn1b-faceplate-rewire.md) routes the web side
  to match — it currently drops both opcodes with a comment pointing here.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. One `cargo test` at a time —
  [[vxn-no-parallel-cargo-test]].

## Close-out (2026-08-26)

Made it live. The premise needed correcting first: the button was never
*live*. Its markup sits inside `keys.js`'s `innerHTML`, below a guard that
returns when `.panel[data-name="Keys"]` is absent — and VXN1b's faceplate has 16
panels, none named Keys, with the panel's CSS already deleted. So no user has
ever seen a RESET button, and nothing was silently marking the patch dirty.
[[0310]] deleted that module; this ticket built the feature instead.

- **The op.** [`SharedParams::reset_layer`](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L268)
  installs [`LayerState::factory_default`](../../vxn-1b/crates/vxn1b-engine/src/state.rs#L90)
  rather than looping descriptor defaults — the slot-depth params must be seeded
  from the matrix or the depth-authority contract (0205) silently breaks. One
  definition of "a fresh layer", shared with `Engine::new` and the state blob.
  Reached by `PatchOp::ResetLayer` ([engine.rs:111](../../vxn-1b/crates/vxn1b-engine/src/engine.rs#L111)),
  parsed at [ui-web lib.rs:154](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L154),
  applied natively at [vxn1b-clap lib.rs:484](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L484)
  and on the web through
  [`vxnc_ui_reset_layer`](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L903)
  → `WebController.resetLayer` → the bridge's `reset_layer` route.

- **Mixer strip: resets.** Decided with the user and recorded in
  `reset_layer`'s doc comment beside the reason. `copy_layer` spares level /
  mute / pan / detune (`COPY_LAYER_EXCLUDED`) because writing a sound onto the
  other layer should not move where that layer sits in the mix; reset is the
  opposite intent, and a "blank" layer still muted at −6 dB hard left reads as
  broken rather than blank. The two ops deliberately do **not** share the
  exclusion list, and the confirmation text says the strip goes.
  Pinned by `shared::tests::reset_layer_also_resets_the_mixer_strip`.

- **Key state: untouched.** Layer enable and the split describe how the layers
  share the keyboard, not either layer's patch. Copy turns Layer 2 on (a copy to
  a silent layer is pointless); reset has no such reason.
  `shared::tests::reset_layer_leaves_key_state_alone`.

- **Scope.** Seven engine tests: every patch param to default, matrix topology
  to `default_patch`, the other layer untouched, key state untouched, reload
  flagged, no gesture flags raised.
  **Divergence from this ticket's spec:** the "both layers reset in Whole" case
  was not built. That clause described vxn-1's model, where Whole means the two
  layers share one patch. VXN1b's Whole *is* Single — Layer 2 is bypassed, not
  shared — so there is no second layer to reset. Reset always acts on the edit
  layer, which is correct in all three of VXN1b's modes.

- **The button.** Preset bar, beside `Copy L1 → L2`
  ([faceplate.html:34](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L34)) —
  same class of destructive whole-patch op, same confirmation. Direction is not
  fixed the way Copy's is, so `syncResetLayerLabel` restamps the label on every
  tab flip and the handler reads the edit layer at click time, not wire time; a
  button reading "Reset L1" that blanks Layer 2 is the expensive version of this
  bug. `__tests__/reset-layer.test.js`, 7 tests, including
  *"reads the edit layer at click time, not at wire time"*.

- **The guard, and what it found.** `tests::every_opcode_the_page_posts_has_a_handler`
  ([lib.rs:1002](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L1002)) scans every
  `op:` literal out of `bridge.js` and drives it through the real
  `vxn_core_ui_web::parse_ui_event`. Verified it bites by pulling the new
  `reset_layer` arm back out — it named the opcode exactly. `set_edit_layer` is
  the only entry in `IN_PAGE_ONLY`, with its reason.

- **Dead echo handlers: five, not one.** The ticket named
  `edit_layer_changed`; a repo-wide sweep for producers found `key_mode_changed`
  and `split_point_changed` are equally unemitted — **zero** Rust producers for
  any of the three, in any crate. All three handlers removed from `dispatch.js`,
  along with four comments describing echoes that cannot arrive (including the
  split-slider's "a `split_point_changed` echo overwrites it", the ticket's
  stale-comment item, which had survived `keys.js`'s deletion by living in
  `dispatch.js` too). Every surviving `ev.kind` handler now has a producer:
  `keys` carries mode + split + link together, exactly as this ticket predicted.

- **Two existing guards caught omissions in passing**, both hand-maintained
  lists: `controller.test.mjs`'s `vxnc_ui_*` wrapper allowlist, and
  `css_covers_every_control_primitive` (during [[0310]]).

Verified: `vxn1b-engine` 305 pass (7 new), `vxn1b-ui-web` 14 pass (1 new) +
Vitest 39 files / 302 pass (7 new), `node --test` 146 pass / **0 skipped**,
`vxn1b-clap` / `vxn1b-wasm` / `vxn1b-web-controller` green.

Manual DAW pass ([[verify-audio-in-reaper]]): **done, works as intended** —
confirmed by the user on 2026-08-26, after this ticket was first closed on test
evidence alone. The close-out originally recorded that check as outstanding;
it no longer is.
