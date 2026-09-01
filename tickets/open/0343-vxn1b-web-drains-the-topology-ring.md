---
id: "0343"
product: vxn-1b
title: "vxn-1b web: nothing drains the topology ring, so it sits permanently full with a resync owed"
priority: low
created: 2026-09-01
epic: E045
depends: []
---

## Summary

[0338](../closed/0338-vxn1b-topology-ring-delete-the-mutex.md) moved matrix
topology onto an SPSC ring so the **native** audio thread stops taking a mutex.
The web build shares the same `SharedParams` — `vxn1b-web-controller` runs the
same `vxn_core_app::Controller<SharedParams>` as the native shell
([lib.rs:61](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L61)) — but its
worklet engine is fed by `vxn1b-wasm`'s own event codec, so **nothing over there
ever calls `drain_topology`**. Grep confirms the only non-test caller is
`vxn1b-clap` ([lib.rs:604](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L604)).

So on the web the ring fills to `TOPO_RING_SLOTS` (64) and stays there, with the
sticky resync flag raised and never serviced. A loose end of
[E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md), which called
non-param topology state across the worklet boundary out as one of the two
genuinely new pieces of engineering in the port.

**This is inert today, and the ticket is `low` for that reason.** The guarded
tables are the only thing the web build reads, they stay correct, and a full ring
costs one refused push per edit. Nothing is wrong with the audio or the UI.

What is wrong is that two public predicates lie off the CLAP path:
[`topology_backlog`](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L441) and
[`topology_resync_pending`](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L447)
report a permanently-full, permanently-owed channel that nobody is going to
service. They exist for tests and diagnostics; a future diagnostic that trusts
them on the web build reads a broken queue where there is only an unused one.
The condition is documented in
[shared.rs](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L39) rather than
fixed, which was the right call for 0338 and is not a resting place.

## Design

Two honest options, and the choice is the ticket's real content:

**A — drain it.** Have the web transport call `drain_topology` against the
worklet's engine, so the ring is the topology channel on both builds and the
predicates mean the same thing everywhere. Right if the wasm codec's matrix path
can be expressed as `TopoMsg` without a second encoding — note
[0339](../closed/0339-fold-wasm-apply-matrix-edit-into-topology.md) already
folded `vxn1b-wasm`'s edit application onto `topology::apply_edit`, so the two
paths share the *applier* and differ only in transport. That makes A smaller
than it looks and is why it is listed first.

**B — don't create it.** Give `SharedParams` a way to be built without the ring
(or with pushes disabled) when it is a UI model rather than a transport, so the
web build never fills a queue it does not read. Cheaper, and honest about the
architecture — but it puts a mode flag on a type that currently has none.

Pick one and record why in the close-out. What is not acceptable is leaving both
predicates meaning different things on different builds with only a doc note to
warn a reader.

## Acceptance criteria

- [ ] On the web build, `topology_resync_pending()` is **false** in steady state
      after a run of topology edits — either because they drained (A) or because
      none were pushed (B).
- [ ] `topology_backlog()` is meaningful on both builds: it answers "records
      waiting for a consumer", and on the web that number does not grow without
      bound.
- [ ] A matrix edit made in the browser still reaches the audio worklet on the
      next block, and preset/state load still crosses as one snapshot — whichever
      option is taken, the behaviour 0338 specified for native holds for web too
      or is explicitly stated not to apply.
- [ ] The stale paragraph in
      [shared.rs](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L39) is replaced
      by whatever is true after this lands, not merely softened.
- [ ] `xtask web` build green and the browser build's matrix overlay still edits
      correctly — a manual check is enough, and say in the close-out that it was
      manual.

## Notes

- **Out of scope: the native path.** 0338's ring works and is verified; nothing
  here should touch `vxn1b-clap`'s drain or the two-store ordering around it.
- The ring is deliberately 64 records, not the web ring's 1024, because
  `TopoMsg::Snapshot` dominates the enum — see the sizing note on
  [`TOPO_RING_SLOTS`](../../vxn-1b/crates/vxn1b-engine/src/topology.rs#L69).
  Option B removes that cost from the web build entirely, which is a small point
  in its favour.
- Related: [[vxn2-mvc-discipline]] (the view/model split this sits inside),
  [[vxn2-web-port-e030]] (the sibling port's divergences — `SharedParams`-as-model
  is called out there too, so this may be a pattern worth naming once rather than
  solving twice).
