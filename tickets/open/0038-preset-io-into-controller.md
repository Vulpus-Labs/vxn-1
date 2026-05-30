---
id: "0038"
title: Move preset corpus IO into the controller
priority: medium
created: 2026-05-30
epic: E009
---

## Summary

Lift the user-preset directory IO and the factory bank readout out of
the editor and into the controller. The controller owns the corpus,
publishes it as a `Arc<Vec<BrowserEntry>>` (or equivalent) via
`ViewEvent`s, and handles every mutation (rename / delete / move /
new folder / save).

## Acceptance criteria

- [ ] Controller has internal state for the browser corpus, refreshed
      on start and after any mutation event.
- [ ] `UiEvent::LoadPreset`, `SavePreset`, `RenamePreset`,
      `DeletePreset`, `MovePreset`, `NewFolder` all handled in the
      controller.
- [ ] After every mutation the controller emits
      `ViewEvent::PresetCorpusChanged` with the new snapshot.
- [ ] Vizia editor's `build_browser` / `reseed_browser` /
      `entry_index_for_user_path` etc. are deleted (they live in the
      controller now); the view binds to the snapshot signal.
- [ ] Existing preset integration tests pass; new test:
      `controller_save_then_list_round_trip` against a tempdir.
- [ ] `vxn-ui` has no `preset_io` import.

## Notes

This ticket is structurally smaller than 0037 because the IO is
already factored well in `vxn-engine::preset_io` — we just move the
*callers* a layer up.

Filter-by-search is part of the view (cheap, derived from corpus +
query string); it stays in the editor. Folder selection is view state;
it stays in the editor. Everything that touches the filesystem moves.

The controller's tick loop now does IO. That's fine — `tick` runs on
the main thread on the host's idle ping; preset IO is fast and not
real-time-sensitive. Long IO would need a worker thread; not yet.
