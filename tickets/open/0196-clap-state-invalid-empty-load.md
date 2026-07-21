---
id: "0196"
product: monorepo
title: "CLAP state load accepts empty/invalid state — should return false (clap-validator state-invalid)"
priority: medium
created: 2026-07-21
---

## Summary

`clap-validator validate` reports one failure on all three synths:

```
state-invalid: The plugin should return false when 'clap_plugin_state::load()'
  is called with an empty state.
  FAILED: The plugin returned true when 'clap_plugin_state::load()' was called
  when an empty state, this is likely a bug.
```

Every synth's `PluginStateImpl::load` reads the whole blob, forwards it to the
controller as `HostEvent::StateLoaded { blob }`, ticks, and returns `Ok(())`
**unconditionally** — even for a zero-byte blob:

- vxn-1 — [vxn-clap/src/lib.rs:552-564](../../vxn-1/crates/vxn-clap/src/lib.rs#L552-L564)
- vxn-2 — `vxn2-clap/src/lib.rs:660-670`
- vxn-3 — `vxn3-clap/src/lib.rs:585-605`

A host handing back an empty/garbage stream (a fresh instance, a truncated
project file) is silently accepted; the plugin keeps whatever params it had and
tells the host the restore succeeded. `state-invalid` wants `false` so the host
knows the load didn't take.

Found during the [[E035]] close-out clap-validator pass (2026-07-21). Unrelated
to E035 (engine-internal); pre-existing.

## Design

The decode already has a fallible form: `vxn_core_clap::state::load_blob`
returns `Result<(), String>` ([state.rs:23](../../crates/vxn-core-clap/src/state.rs#L23)),
but the CLAP `load` paths bypass it — they push the raw blob through the
controller channel and drop the result on the floor.

Make `load` reject a blob it can't apply:

1. **Minimum: reject empty.** `if blob.is_empty() { return Err(PluginError::
   Message("empty state")); }` before sending `StateLoaded`. Clears the
   validator failure.
2. **Better: reject undecodable.** Validate the blob decodes (round-trip through
   `load_blob` / whatever the controller's `StateLoaded` handler uses) *before*
   committing it, and return `Err` on failure so a truncated/garbage stream
   doesn't half-apply. Decide whether the controller should decode-then-apply
   (so `load` can see the verdict) rather than the current fire-and-forget
   `try_send` + `tick`.

Apply the same fix to all three synths. If the check is identical, consider a
shared helper in `vxn-core-clap` (the blob format is already shared there) —
coordinate with [[0195]] which is also consolidating into the core crates.

## Acceptance criteria

- [ ] `clap-validator validate` `state-invalid` passes for vxn-1, vxn-2, vxn-3
      (empty state → `load` returns false / `Err`).
- [ ] A valid saved state still round-trips (`state-reproducibility-*` stay
      green — they currently pass; don't regress them).
- [ ] Ideally: a truncated/garbage (non-empty) blob is also rejected rather than
      partially applied, or a documented decision on why empty-only is enough.
- [ ] `cargo test` green across the three `*-clap` crates.

## Notes

- vxn-1 was verified failing (17 pass / 1 fail) on 2026-07-21; vxn-2/vxn-3 not
  re-run but share the identical `Ok(())`-always pattern — confirm all three
  under the validator when fixing.
- Keep the fix on the main thread (this is `PluginStateImpl`, not the audio
  thread); no realtime constraint.
