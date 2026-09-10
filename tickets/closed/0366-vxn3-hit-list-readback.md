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

## Close-out (2026-09-10)

Landed as an **MVC correction**, not just a readback. The first implementation
answered the ticket literally — an audio→main pull mailbox that let the page ask
the engine what it held — and was rejected for pointing the direction of truth
the wrong way. vxn-3 is MVC: the model is main-thread and authoritative, the
faceplate is a view over it, and the audio thread works from an internal copy kept
in step by deltas. Truth flows main → audio, never back. That principle is now
stated at the top of [`io.rs`](../../vxn-3/crates/vxn3-engine/src/io.rs).

So the "readback" is a plain main-thread read. The audio thread is not involved in
deciding what the editor draws, and the mechanisms the first attempt needed to make
an RT round-trip safe — a request handshake, an `IDLE → REQUESTED → READY` state
word, a repeat-until-answered retry, and a `pending` strip that refused edits until
the engine replied — are all deleted rather than left inert. None of them has a
reason to exist once the data the view wants is already on its own thread.

**The four design points, as decided.**

- **What crosses the channel.** Nothing, in the ordinary case.
  [`PatternStore`](../../vxn-3/crates/vxn3-engine/src/io.rs) holds the authoritative
  per-lane `Pattern` on the main thread, beside `FlavourStore` and `TrackKinds` and
  for the same reason. The page is **built from the model** — the hit list is in the
  config JSON the faceplate is constructed with, so a reopened editor starts in
  agreement instead of starting empty and asking. The `Mutex` is uncontended and
  exists only to keep `EngineIo` `Sync`; the audio thread never takes it, which is
  what makes the read free of any RT consideration at all.
- **When it is sent.** The view is told exactly once per **replacement**, never per
  edit. Every ordinary mutation of a lane originated as a `Vxn3UiCustom::Edit` from
  the view itself, so echoing it back would tell the page what it just said and
  fight a gesture in flight with a stale snapshot. `PatternStore::set` marks the
  lane dirty and `announce_replaced_lanes` drains that into
  `Vxn3ViewCustom::Lane` — silent on an ordinary tick, and speaking up for a state
  restore, which is the one change the view cannot already know about.
- **Fire-order index stability.** No stable identity, documented on
  [`Vxn3ViewCustom::Lane`](../../vxn-3/crates/vxn3-app/src/lib.rs). The index stays
  positional. View and model resolve a fire time with the same arithmetic (0353), so
  their lists agree index for index as long as they *started* in agreement — which
  building the page from the model guarantees. A per-hit id would change the wire
  form of every hit-keyed opcode to buy what starting in agreement already gives.
  Applying a `Lane` event is therefore a **hard resync**: it invalidates any index a
  gesture in flight is holding, and the view drops that gesture.
- **The `NO_COLOUR` sentinel.** `rgb` is JSON `null` for an uncoloured hit and a
  triple otherwise — the decoded vector the macro slots actually receive, so the page
  shows what the hit will *do*. Black stays `[0, 0, 0]`. Placed hits carry
  `rgb: null` too, so both paths have one shape for 0355 to render.

**The full flush.** In scope here because a model-authoritative design needs a way to
get a *replaced* model into the engine, and deltas are the wrong shape for it: the
edit queue is `Copy` commands with no verb for marker geometry, and 64 hits × 8 lanes
would overrun `QUEUE_CAP` regardless. So `EngineIo::flush_lane` sends the whole
`Pattern` out of band through a bounded SPSC `FlushRing` (`FLUSH_CAP = 4`, inline
slots, a fixed-size `Copy` on both sides and never an allocation), paired with an
`EngineCommand::LoadPattern` marker in the edit queue that fixes **where in the delta
stream it lands** — a flush and the edits around it have one order, not two. The
queue slot for the marker is reserved *before* the pattern is committed to the ring
(`EditQueue::can_push`), so the two can never come apart; a flush that cannot mark
does not push.

**Per acceptance item.**

- Hit list agrees after an editor reopen — `vxn3_ui_web::tests::html_is_built_from_the_model`,
  `page_reads_the_model_through_one_reader`. The reopen case is now correct by
  construction rather than by protocol: the page cannot start out of agreement because
  its initial state *is* the model.
- After a `clap.state` restore — `vxn3_clap::tests::state_restore_leaves_model_and_engine_in_agreement`,
  `a_full_flush_replaces_a_running_engines_lane`, `a_replaced_lane_is_announced_to_the_view`,
  `a_new_engine_seeds_its_lanes_from_the_model`. **Still scoped, as the ticket's Notes
  anticipated:** hits are not serialised — the blob carries params, kinds and flavours
  (0348). What changed is that the mechanism a restore will need now exists, so
  serialising patterns becomes the only missing half rather than the missing half plus
  a channel to carry it. **Pattern serialisation is separate work and is not in this
  ticket.**
- No path clears or discards the engine's pattern —
  `tests/faceplate_io.rs::a_readback_does_not_disturb_the_pattern` (hits *and* grid
  identical before and after).
- A hit-keyed edit addresses the right hit, over a lane not in insertion order —
  `tests/faceplate_io.rs::a_readback_makes_the_next_hit_keyed_edit_address_the_right_hit`.
- `NO_COLOUR` survives, distinct from black —
  `vxn3_ui_web::tests::no_colour_survives_the_readback_distinct_from_black`
  (uncoloured reads `null`, never black).
- No audio-thread allocation — `tests/faceplate_io.rs::pattern_readback_is_allocation_free`,
  `flushing_every_lane_is_allocation_free`, `flush_ring_is_bounded`. The
  `tests/{groove,pattern,plocks}.rs` traps stay green.
- Index stability documented — on `Vxn3ViewCustom::Lane`, with the hard-resync
  consequence spelled out.

**Model/engine agreement is one implementation, not two.** Both the main-thread model
and the audio thread's copy apply edits through the same `apply_pattern_command`, so
"in step" is a property of shared code rather than of two hand-maintained code paths.
Pinned by `model_and_engine_copy_stay_in_step_under_the_same_commands`,
`the_model_and_the_engines_copy_hold_the_same_lane`,
`a_lane_edit_lands_in_the_model_and_on_the_queue`,
`an_edit_the_queue_rejects_does_not_advance_the_model` (a dropped delta must not
advance the model, or the two diverge silently), `non_lane_commands_do_not_touch_the_model`
and `command_track_routing_covers_the_vocabulary`.

**Verified structurally, not visually.** No browser automation was available.
`cargo test --workspace` is green; `cargo build` is clean across the four vxn-3 crates.
The rendered faceplate (`cargo run -p vxn3-ui-web --example preview`) carries the
model-seeded `lanes` in its config and contains no trace of the deleted mechanisms
(`strip.pending`, `laneReady`, `request_lane`, `PatternMirror` — zero occurrences
each). The first attempt's DOM-shim walkthrough was **not** carried forward: every
behaviour it exercised was a property of the request/retry protocol that no longer
exists. `the_view_has_no_readback_request` and `ordinary_edits_are_not_echoed_to_the_view`
pin its absence.

**Adjacent, pre-existing, not fixed here.** The page still force-pushes its default kit
(`assign_voice` ×8 on load, [app.js](../../vxn-3/crates/vxn3-ui-web/assets/app.js)), so a
restored pattern comes back with its hits intact but its voices overwritten. Voice
readback is its own ticket; 0366 makes the path reachable for the first time.
