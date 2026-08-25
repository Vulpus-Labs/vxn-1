---
id: "0302"
product: vxn-1b
title: "vxn1b-clap: drain the bits, delete the poll and both memo echoes"
priority: medium
created: 2026-08-25
epic: E046
depends: ["0301", "0306"]
---

## Summary

Ticket of [E046](../../epics/open/E046-dirty-bitset-pump-vxn1-vxn1b.md): the
native shell moves onto [[0301]]'s bits. This is where the epic pays out — three
change-detection mechanisms collapse into one.

## Design

In [`on_timer`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L456), after
`ctrl.tick(...)` and the `view_rx` drain, drain the model's bits and push:
`ParamChanged` per value bit (with
[`sync_aware_display`](../../vxn-1b/crates/vxn1b-engine/src/sync.rs#L105) and the
[`rate_partner_clap_id`](../../vxn-1b/crates/vxn1b-engine/src/sync.rs) refresh —
that rule survives the mechanism change), a whole-table `MatrixSnapshot` on the
matrix word, and a `KeyState` echo on the view-key bit.

### What goes

| ref | why |
|---|---|
| [`push_param_diffs`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L303) + `last_seen` | the bits are the change marker |
| [`push_matrix_echo`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L350) + `last_matrix` | ditto, and it was the bespoke-push workaround ADR 0003 names |
| [`push_key_echo`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L379) + `last_key` | ditto |
| the memo-clearing on `take_editor_ready_flag` ([lib.rs:504-511](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L504-L511)) | with no memos, `EditorReady` re-seeding is `mark_all` |
| `set_echo_param_writes` default | flipped to `false` |

Keep the meter and scope pushes exactly as they are — telemetry frames are
audio-thread data on their own cadence, not model state, and they never had a
dirty bit.

### The editor-attach path is the subtle one

Today a re-attached page is re-seeded by clearing both memos so the next push is
unconditional. Under the pump the equivalent is `mark_all()` on the model when
`take_editor_ready_flag()` returns true. Get this wrong and the symptom is
precisely the bug 0221 fixed: a session reloaded with Layer 2 on plays dual while
the switch reads dark. It is not covered by any automatic test — the flag only
fires from a real editor attach.

### Ordering

Controller echo is off, so `view_rx` now carries only non-param events
(`PresetLoaded`, `Status`, corpus) and the bits carry everything model-backed.
The two-pushes-can-double comment at
[lib.rs:515-518](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L515-L518) stops being
true — delete it rather than leaving it to mislead. Note this also means the
WebView's dedupe-by-id in `flush_view_events` is no longer load-bearing here;
leave it (vxn-1 still needs it until [[0305]]) but don't rely on it.

## Acceptance criteria

- [ ] `on_timer` drains value / matrix / key channels; the three `push_*` diff
      fns and their memo fields are deleted, not left unused.
- [ ] `echo_param_writes(false)`, with a test asserting one `ParamChanged` per
      model write — not zero, not two.
- [ ] A sync-toggle flip still re-pushes its rate partner's display.
- [ ] Host automation from the DAW still reaches the editor (this is the case
      `push_param_diffs` existed for — the audio thread writes the store
      directly via `LocalParams::publish`).
- [ ] Preset load, host state load, host undo and `copy_layer` each move the
      matrix / key state on screen with no bespoke push.
- [ ] Editor close → reopen re-seeds params, matrix and key state.
- [ ] `cargo test -p vxn1b-clap` + `cargo test --workspace` green.
- [ ] Manual DAW pass ([[verify-audio-in-reaper]]): automate a param from the
      host, load a preset, hit undo, close and reopen the editor. None of this
      has an audible signature, so tests alone do not close this ticket.

## Notes

- Land as its own commit, separate from [[0301]] — the engine change is additive
  and the deletions are where the risk is.
- The web controller keeps its own path until [[0303]]; both shells drain the
  same model, so they can differ for a while without conflict.
- One `cargo test` at a time — [[vxn-no-parallel-cargo-test]]. No `cargo fmt` —
  [[vxn-no-cargo-fmt]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].
