---
id: "0366"
product: vxn-3
title: "Faceplate hit-list readback — the editor must see the pattern it is editing"
priority: high
created: 2026-09-09
epic: E050
depends: ["0353"]
---

## Summary

Corrective ticket of [E050](../../epics/open/E050-vxn3-continuous-lane-editor.md),
opened after [0353](../closed/0353-vxn3-faceplate-lane-strip-diamonds.md) landed
the continuous lane strip and recorded this gap in code.

The faceplate's hit list is **seeded empty and never read back**. The view
channel carries only the playhead —
[`Vxn3ViewCustom::Playhead`](../../vxn-3/crates/vxn3-app/src/lib.rs#L120) is its
sole variant — so the page has no way to learn what a lane already holds.

That is correct for a fresh instance and wrong for every other case: a GUI
reopen, or a `clap.state` restore into a lane that already has hits. The two
lists then disagree, and since
[0353's edit vocabulary is hit-keyed by fire-order index](../../vxn-3/crates/vxn3-engine/src/io.rs),
**every subsequent edit addresses the wrong hit** — a drag moves someone else's
diamond, a delete removes the wrong one.

## Design

The fix is a hit-list readback on the view channel, alongside the playhead.

The obvious wrong fix, and the reason this is a ticket rather than a patch:
**clearing the engine's lanes on editor load** would buy agreement instantly and
silently discard a restored pattern. 0353 declined to do that deliberately. Any
implementation must leave the engine's pattern authoritative and make the page
follow it.

Points that need deciding rather than falling out of the code:

- **What crosses the channel.** A full hit list per lane is `MAX_HITS = 64` ×
  8 tracks of `(beat, sub, f, nudge, y, rgb, note, velocity, probability,
  retrig)`. The playhead today is `[AtomicU32; N_TRACKS]` — this is a different
  order of payload and does not fit the same mechanism.
- **When it is sent.** On editor open is the minimum. Whether every engine-side
  pattern mutation also publishes (making the page a pure view) or the page keeps
  its optimistic local copy between readbacks is the real design choice.
- **Fire-order index stability.** 0348 keeps hits sorted by fire time and
  re-sorts on insert and on any position or geometry edit, so an index is only
  valid until the next edit. A readback fixes the *reopen* case; it does not by
  itself make a stale index safe. Decide whether hits need a stable identity, or
  whether the page must re-read after every edit that can re-sort.
- **The `NO_COLOUR` sentinel** from [0351](../closed/0351-vxn3-per-hit-rgb-macro-vector.md):
  `Hit::rgb` defaults to `-1.0` per channel, meaning "no colour", which is not a
  renderable value. The readback must carry that distinction rather than
  flattening it to black — black is a real macro vector that sends zero.

## Acceptance criteria

- [ ] The faceplate's hit list agrees with the engine's after an editor reopen
      over a lane holding hits.
- [ ] The same holds after a `clap.state` restore.
- [ ] No path clears or discards the engine's pattern in order to reach
      agreement.
- [ ] After a readback, a hit-keyed edit (drag, delete, quantise) addresses the
      hit the user is pointing at — asserted against a lane whose hits are not in
      insertion order.
- [ ] `NO_COLOUR` survives the round trip and is distinguishable from
      `rgb = [0, 0, 0]`.
- [ ] The readback does not allocate on the audio thread and does not block it;
      `tests/{groove,pattern,plocks}.rs`'s allocation traps stay green.
- [ ] Index-stability behaviour is documented on the view type, whichever way it
      is resolved.

## Notes

Hits are **not serialised yet** — the per-track patch blob redefined in 0348
carries params, flavours and grid geometry, not the hit list. So the
`clap.state` half of this ticket may need that first, or may scope to "restore
puts the engine and page in the same state" once it exists. Check before
estimating.

Blocks nothing in E050's remaining tickets ([0350](0350-vxn3-y-centre-curve.md),
[0352](0352-vxn3-groove-object-pool-assignment.md),
[0354](0354-vxn3-faceplate-marker-drag-swing.md),
[0355](0355-vxn3-faceplate-palette-colour-render.md),
[0356](0356-vxn3-faceplate-y-curve-groove-pool.md)) but every one of them adds
editor state that will have the same problem, so landing it early costs less than
retrofitting it four times.

The gap is recorded in code at
[`vxn3-ui-web/src/lib.rs`](../../vxn-3/crates/vxn3-ui-web/src/lib.rs) (module
docs, ~L22) and
[`app.js`](../../vxn-3/crates/vxn3-ui-web/assets/app.js) (~L226, ~L235).
