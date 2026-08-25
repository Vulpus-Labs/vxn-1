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
