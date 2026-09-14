---
id: "0384"
product: vxn-4
title: "vxn-4 clap.state v2 carries the patch payload"
priority: medium
created: 2026-09-10
epic: E052
depends: ["0382", "0383"]
---

## Summary

Teach `clap.state` to save the patch, not just the eleven host params. The
current blob is a header plus eleven floats, and its own module doc names this
ticket: *"When patches become editable this blob gains a payload and a version
bump; the header is shaped for that"*
([state.rs:20-24](../../vxn-4/crates/vxn4-clap/src/state.rs#L20-L24)).

## Acceptance criteria

- [ ] Blob version bumps to 2 and carries the full patch alongside the param
      values.
- [ ] A **v1 blob still loads**: the params restore as they do today and the
      patch resolves to the factory patch that the stored patch index names.
      An old project must not fail to open.
- [ ] A v2 blob written by a build with fewer descriptor ids loads into a build
      with more; the missing fields take their descriptor defaults. The existing
      `n_params` forward-compat story extends to the patch payload rather than
      being replaced by a stricter one.
- [ ] Save is deterministic — identical state produces identical bytes, which
      `clap-validator` requires and the current implementation already
      guarantees ([state.rs:31-33](../../vxn-4/crates/vxn4-clap/src/state.rs#L31-L33)).
- [ ] A corrupted or truncated blob fails the load outright rather than
      restoring half a project.
- [ ] Restore crosses to the audio thread as **one snapshot** on the topology
      ring from [0382](0382-vxn4-invert-patch-ownership.md), not as a stream of
      field edits, and is coherent against any params landing in the same tick.
- [ ] Round-trip test through the real CLAP calls via `clack-host`, in-process,
      as the existing lifecycle test does.

## Notes

Two encodings are possible for the payload and the choice is worth making
explicitly: the TOML text from [0383](0383-vxn4-sparse-toml-preset-codec.md)
embedded as a string, or a binary encoding of the same fields.

TOML is strongly preferred. It gets forward and backward compatibility for free
from being sparse and name-keyed — which is exactly the property a host blob
needs and the hardest one to retrofit onto a binary layout — and it means one
format to test rather than two. vxn-1b's blob is binary for historical reasons,
not because binary won. Size is not a real constraint: a sparse patch is a few
kilobytes of text and hosts store far larger blobs routinely.

The `positional scheme is stable` invariant behind the current forward-compat
story ([state.rs:16-21](../../vxn-4/crates/vxn4-clap/src/state.rs#L16-L21))
applies only to the eleven CLAP ids and is unaffected by this ticket — the patch
payload is name-keyed and has no positional invariant to preserve. Say so in the
module doc so the two schemes are not confused later.

## Close-out (2026-09-14)

- [state.rs](../../vxn-4/crates/vxn4-clap/src/state.rs) writes version 2: the v1
  header and value array unchanged, then `payload_len: u32` and the 0383 TOML
  text. `the_blob_is_the_shape_the_header_claims`. TOML over a binary layout, as
  the ticket argued — sparse name-keying gives forward *and* backward
  compatibility for free (`a_payload_from_a_different_build_loads_either_way`).
- **v1 still opens**: values restore as before, then the body resolves to the
  factory patch the stored index names — unconditionally, so a v1 restore
  replaces a body the store had drifted from
  (`a_v1_blob_still_loads_and_resolves_its_factory_patch`,
  `a_v1_blob_replaces_a_body_the_store_already_had`,
  `a_v1_project_blob_still_opens` through the real host calls). Confirmed
  separately against a v1 blob built by hand rather than derived from a v2 save,
  since no real v1 file was ever produced by this build's writer.
- Deterministic (`saving_the_same_state_twice_gives_the_same_bytes`),
  all-or-nothing on failure (`rubbish_is_rejected_rather_than_half_loaded`,
  `a_payload_that_is_not_text_is_rejected`), tail-tolerant both directions
  (`a_longer_blob_loads_and_ignores_the_tail`,
  `a_shorter_blob_loads_and_leaves_the_rest_alone`).
- Restore crosses as **one** snapshot on 0382's ring —
  `a_restore_crosses_as_one_snapshot` asserts `topology_backlog() == 1`.
- **Patch-index ownership decided**: the CLAP `patch` param owns the selection,
  the store owns the body, joined by `rebase_selection`, which fires when the
  selection moves and never because the body drifted. The reasoning is in the
  module doc — the selection is written on the audio thread, and the store's
  bulk install takes a main-thread-only mutex by construction, so the two can
  only be joined where the main thread runs.
  `the_store_rebases_on_a_new_selection_and_only_on_that`.
- This is the ticket that made [0382](0382-vxn4-invert-patch-ownership.md)
  load-bearing: `sync` is now called in `activate` and at the top of `process`.
  The bit-identity net still passes with it live.
- Engine addition: `SharedParams::patch_snapshot()`, reading topology from the
  mirror then depths from the atomics — a mirror carries a stale depth after a
  `set`, so comparing raw mirrors would have been wrong.
- `vxn4-clap` 33 unit + 11 integration; `vxn4-engine` 140 unit + 2 integration.
