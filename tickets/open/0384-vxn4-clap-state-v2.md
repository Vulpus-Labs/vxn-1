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
