---
id: "0296"
product: monorepo
title: "rebuild() silently resets every param to its default in the vxn-1 and vxn-2 web ports"
priority: medium
created: 2026-08-25
epic: null
depends: []
---

## Summary

`WebHost.rebuild()` exists so the audio graph can be re-created over the **same**
SABs when an AudioContext's immutable sample rate has to change — a device switch
to hardware at a different rate. Its own doc comment says so:

> Rebuild the graph at a (possibly new) sample rate, reusing the SAME SABs so
> transport/param state survives.

It does not. `rebuild()` calls `start()`, and `start()` calls
`_seedStoreFromDefaults()` unconditionally, which bulk-writes the engine's
defaults over the whole param store. Every param the user had touched is reset.

- vxn-1: [coordinator.mjs:445](../../vxn-1/crates/vxn-wasm/web/coordinator.mjs#L445)
  → [:213](../../vxn-1/crates/vxn-wasm/web/coordinator.mjs#L213)
- vxn-2: [coordinator.mjs:313](../../vxn-2/crates/vxn2-wasm/web/coordinator.mjs#L313)
  → [:140](../../vxn-2/crates/vxn2-wasm/web/coordinator.mjs#L140)

The seeding itself is correct and necessary: the store's slots are
zero-initialised and the worklet's first fold is NaN-seeded, so it applies every
id — an unseeded store would write `0.0` over every param and silence the
instrument. That is a **first-boot** problem. After the first boot the store holds
the authoritative patch, and re-seeding it is destructive.

Found by a VXN1b test asserting the documented behaviour
([0289](0289-vxn1b-worklet-coordinator.md)); VXN1b's coordinator guards the seed
with a `_storeSeeded` flag. The same one-line guard applies to both other ports.

## Why it has not been noticed

`rebuild()` only fires on a sample-rate-changing device change, which is rare and
manual — plug in an interface that runs at 44.1 when the context came up at 48.
The failure then looks like "the patch reset itself when I changed audio device",
which is easy to blame on the browser.

## Acceptance criteria

- [ ] `_seedStoreFromDefaults` runs once per `WebHost`, in both ports.
- [ ] A test in each port: set a param, `rebuild()`, assert the value survived.
- [ ] A test in each port: a fresh `WebHost` still seeds, so the first boot is
      not silent.
- [ ] Both web suites green, 0 skipped.

## Notes

- Deliberately not fixed inside the VXN1b ticket that found it — it is a vxn-1
  and vxn-2 product bug, and burying it in a VXN1b diff would hide it, the same
  mistake [0285](0285-web-param-mirror-drift.md) warns about.
- VXN1b's guard is the reference implementation.
