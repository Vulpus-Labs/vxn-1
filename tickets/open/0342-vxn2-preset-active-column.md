---
id: "0342"
product: vxn-2
title: "vxn-2 preset format loses a route's on/off switch: muting, saving and reloading turns it back on"
priority: medium
created: 2026-09-01
epic: null
depends: []
---

## Summary

VXN2's TOML preset format has no column for a matrix route's on/off switch, so
switching a route off, saving the preset and loading it back **turns the route
back on**. Silent data loss, and the toggle it destroys is exactly the one
[0333](../closed/0333-share-slot-and-route-compilation.md) added to make A/B-ing
a route non-destructive.

The state blob is fine — `MatrixRowRaw` carries `active`
([shared.rs:721](../../vxn-2/crates/vxn2-engine/src/shared.rs#L721)) and it is a
real bit in the packed word. Only the **TOML preset** path drops it:

- [`MatrixRowFile`](../../vxn-2/crates/vxn2-engine/src/preset.rs#L130) has
  `slot`, `source`, `dest`, `curve`, `depth`, `scale-src`, `scale-shape` — and no
  `active`.
- [`matrix_rows_file`](../../vxn-2/crates/vxn2-engine/src/preset.rs#L248) never
  reads `row.active`.
- The reader **synthesises** it back from the endpoints:
  [`active: source != 0 && dest != 0`](../../vxn-2/crates/vxn2-engine/src/preset.rs#L499).
  That line is the pre-0333 fold — "switched off" used to *be* `source = None` —
  and it is now wrong for any wired-but-off slot.

Found while closing 0333 and flagged there; VXN1b already persists the flag
([preset.rs:150](../../vxn-1b/crates/vxn1b-engine/src/preset.rs#L150), written
with `skip_serializing_if = "is_true"`), so this is also a gratuitous divergence
between two formats that otherwise agree on what a slot is.

## Design

Add `active` to `MatrixRowFile` on VXN1b's pattern, which is the one that
already solves the compatibility problem:

```rust
#[serde(default = "default_enabled", skip_serializing_if = "is_true")]
active: bool,
```

`default = true` on read, so every preset written before this key existed loads
exactly as it does today; `skip_serializing_if` on write, so a preset whose
routes are all on round-trips **byte-identically** and the factory bank does not
churn. Only a genuinely-muted route grows a key.

Then the reader takes the column instead of deriving it, and
`active: source != 0 && dest != 0` at
[preset.rs:499](../../vxn-2/crates/vxn2-engine/src/preset.rs#L499) becomes
`active: active && source != 0 && dest != 0` — keeping the endpoint check, since
a row naming `none` is inert whatever the column says.

## Acceptance criteria

- [ ] A wired route with `active = false` survives a write → read round trip
      switched **off**. Test it in `preset.rs`'s own suite, against a table with
      one off route and one on route into the same destination, so a reader that
      loses the flag shows up as an audible depth change rather than a field
      mismatch.
- [ ] A preset file written before this ticket (no `active` key) loads with
      every route **on** — the `serde` default. Pin it with a literal TOML
      fixture in the test, not a round trip, or the assertion tests the writer.
- [ ] Every factory preset re-serialises **byte-identically** after the change.
      They have no muted routes, so `skip_serializing_if` must keep the key out
      entirely; a diff in the bank means the write path grew a key it should not
      have.
- [ ] The reader keeps rejecting a row whose `source` or `dest` is `none`,
      `active = true` notwithstanding.

## Notes

- **Out of scope: the state blob.** It already carries the bit; this is the TOML
  path only.
- The two formats stay divergent on purpose in general — ADR 0003 §4 and
  §"Alternatives" keep the wire encodings per-synth, and nothing here changes
  that. This is one missing column, not a convergence.
- Worth checking whether the **web** preset path (`vxn2-wasm`, factory bank as
  `factory.bin`) reads through the same `read_preset`; if it has its own decode
  it needs the same column, and if it does not, say so in the close-out.
- Related: [[vxn2-preset-system]], [[vxn2-factory-preset-legal-posture]] (the
  bank is generated, so a churn diff there is loud).
