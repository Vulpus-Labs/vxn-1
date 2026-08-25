---
id: "0300"
product: vxn-1b
title: "ADR: dirty-bitset Model→View pump for VXN1b (and the key_dirty two-reader split)"
priority: medium
created: 2026-08-25
epic: E046
depends: ["0299"]
---

## Summary

Second ticket of [E046](../../epics/open/E046-dirty-bitset-pump-vxn1-vxn1b.md),
and the decision gate: **write the ADR before moving code, and be willing to
conclude "don't".**

VXN1b inherited vxn-1's poll-and-diff idiom by forking vxn-1's shell six weeks
after vxn-2's [ADR 0003](../../vxn-2/adrs/0003-dirty-bitset-diff-pump.md)
settled the question the other way. Grep of all three VXN1b ADRs for
`bitset` / `poll` / `last_seen` / `diff pump` returns nothing — it was never
argued either way. This ADR argues it.

## Design

The ADR must land three things ADR 0003 does not cover, because they are VXN1b's
own shape:

### 1. `key_dirty` has two readers, and that is the actual bug

[shared.rs:65](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L65) is a dirty
flag for a non-CLAP field — the right instinct — but
[`take_key_state`](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L290) is
consumed by the **audio thread** to re-sync the engine, so the **view** cannot
use it. `push_key_echo` says so out loud
([lib.rs:376-378](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L376-L378)) and
memo-diffs instead. vxn-2's bitsets are single-reader by contract.

Decide the split explicitly: an audio-consumer flag (engine re-sync, keeps
`take_key_state`'s semantics) and a view-consumer bit drained by the tick.
Note that the two have genuinely different clear points — the audio thread may
not have run when the tick fires, and vice versa — so this is two flags, not one
flag read twice.

### 2. Matrix topology lives behind a `Mutex`, not in atomics

vxn-2's matrix is `AtomicU32` slots with per-slot dirty bits. VXN1b's is
`Mutex<[MatrixTable; 2]>` ([`edit_matrix_slot`](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L271)).
Per-slot resolution buys nothing when the view push is a whole-table
`MatrixSnapshot` anyway (ADR 0003 reached the same conclusion for 16 rows) —
so one dirty word, or even a bool per layer. Say which and why.

### 3. What replaces `reload`

`reload` ([shared.rs:60](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L60)) is
the audio thread's "re-read topology" signal and is **not** a view channel. It
stays. The ADR should be explicit that view bits and audio-resync flags are two
axes that happen to be flipped by the same writes, or the next reader will try
to unify them and break `take_reload`.

### The case for declining

Worth writing down honestly, because it is not weak: every symptom is already
worked around and shipped. Two memo-diffs and a poll over 185 floats at 60 Hz
cost nothing measurable, and VXN1b is a released product. The counter is
coverage, not cost — the memo-per-field pattern has already been applied twice
and the field list is still growing (LFO 2 link, layer copy, scope tap). If the
ADR concludes the churn is not worth it, close E046 with that written down; that
is a real outcome, not a failure.

## Acceptance criteria

- [ ] `vxn-1b/adrs/0004-*.md` exists with Status Accepted or Rejected — either
      is a valid close.
- [ ] It states the `key_dirty` two-reader split as a decision, with the clear
      points named.
- [ ] It states the matrix dirty granularity and why.
- [ ] It states that `reload` is an audio-resync channel and survives untouched.
- [ ] It lists what dissolves (`push_param_diffs`, `push_matrix_echo`,
      `push_key_echo`, the memo fields, echo-on) and what survives (gestures,
      `reload`, mid-drag suppression in the view).
- [ ] It records the consequence for the web port: 0290's three explicit
      broadcasts and its pack-time display recompute become deletable
      ([[0303]]).
- [ ] If Rejected: E046 is closed unbuilt and 0290's workarounds are marked
      permanent rather than provisional.

## Notes

- Model the structure on [ADR 0003](../../vxn-2/adrs/0003-dirty-bitset-diff-pump.md)
  — Context / Decision / What dissolves / What survives / Consequences /
  Correctness. Its race-window analysis transfers verbatim and should be cited,
  not re-derived.
- VXN1b ADR numbering is per-product and currently at 0003.
- Blocks 0301.
