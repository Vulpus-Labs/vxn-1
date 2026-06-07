---
id: "0031"
title: "UI-echo: LocalParams publish + ViewEvent::Set diff loop"
priority: high
created: 2026-06-06
closed: 2026-06-07
epic: E003
---

## Summary

Close the UI-echo loop: audio-thread automation written into
`SharedParams` via `LocalParams::publish` (already in place from
E002) becomes a `ViewEvent::ParamChanged` to the page within one
tick. The controller's event queue covers UI-driven writes and
host parameter events on the main thread; this ticket adds the
diff pump that catches audio-thread writes the controller never
saw.

The diff pump is the `push_param_diffs` body 0024 stubbed in
`VxnMainThread::on_timer`: compare every CLAP id against
`last_seen`, emit `ParamChanged` for any drift, update
`last_seen`. Sync-aware display strings come along free.

## Acceptance criteria

- [x] `vxn2-clap::VxnMainThread::push_param_diffs` implemented:
      iterate every CLAP id; for each, fetch `plain` via
      `ParamModel::get`, compare against `last_seen[i]`. NaN-
      aware so the initial all-NaN seed broadcasts the table on
      first tick (matches VXN1's pattern).
- [x] For each drifted id: compute `norm` via
      `ParamModel::get_normalized`, build a `display` string via
      a `sync_aware_display(params, id, plain)` helper, push
      `ViewEvent::ParamChanged { id, plain, norm, display }`
      directly onto the editor handle (NOT through the
      controller — those updates aren't gestures, just echoes).
- [x] Sync-aware display: when `id` is `lfo1_rate` and
      `lfo1_sync` is on, render the matching subdivision label;
      same for `delay_time` / `delay_sync`. A 4-entry static
      lookup is enough (LFO2 isn't synced in VXN2;
      reverb / master aren't synced).
- [x] When a sync flip (`lfo1_sync` or `delay_sync`) crosses the
      diff, the rate / time partner's display label re-emits
      even if its plain value didn't change — same trick
      `vxn-clap`'s `force_rate_refresh` uses.
- [x] Gesture suppression: if `ParamModel::gesture(id)` is true
      (the page is actively dragging this fader), skip the diff
      emit for that id. Host automation can still drive the
      param's value into `SharedParams`; the page won't get
      yanked back until the gesture ends.
- [x] Manual smoke: in a host with a written automation curve
      on `lfo1_rate`, open the editor — the LFO1 rate fader
      moves smoothly during playback. Flip the sync toggle off
      during playback; the rate fader's readout switches from
      "1/4" to "2.40 Hz" within one tick.
- [x] No allocations in `push_param_diffs` beyond the
      `String::new()` for the display (small + amortised — VXN1
      runs the same shape at 60 Hz without issue).

## Notes

- The audio-thread side is already done by E002's
  `LocalParams::publish`. This ticket adds only the main-thread
  diff loop. Don't touch the audio path.
- `last_seen` lives on `VxnMainThread` (allocated in 0024's
  `new_main_thread` as `vec![f32::NAN; TOTAL_PARAMS]`).
  Resizing if `TOTAL_PARAMS` ever changed at runtime isn't a
  thing — it's a compile-time const.
- The diff loop is O(N) per tick where N = 343 params. At 60 Hz
  that's ~20k atomic loads per second — well inside budget.
  Optimise only if a profile flags it.
- Sync partner lookup goes in `vxn2-app::sync` if we want a
  module; for now a `match` on the CLAP id inside
  `sync_aware_display` is simpler and fits in <30 lines.
- The `last_seen` initial NaN seed means the first tick after
  open broadcasts 343 ParamChanged events into the buffer. The
  WebView backend's per-tick dedupe (`dedup_param_changes`)
  doesn't help here (each id is unique), so the first
  evaluate_script after open ships a ~150 KB payload. That's
  within the 100 KB default chunk size × ~2 chunks — the
  batcher already splits, no work needed.
