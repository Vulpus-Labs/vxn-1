---
id: "0020"
product: vxn-1
title: UI-web hygiene batch — prototypes, CSS, single-slot cb
priority: low
created: 2026-06-10
epic: E011
---

## Summary

Batch of small vxn-ui-web items from the 2026-06-10 review.
Depends on 0016: the orchestration tests land first so this
cleanup is pinned by them. No behaviour changes intended.

## Acceptance criteria

Assets:

- [ ] `assets/prototypes/wave-knob.html` removed from the
      crate tree (never `include_str!`'d; dead weight that
      drifts from the real `makeWave`). Archive outside the
      crate if wanted.
- [ ] `faceplate.css:567` empty `.panel[data-layered] {}`
      ruleset removed.
- [ ] Literal hex values in the browser-panel CSS sections
      (`#2a2a2a`, `#1c1c1c`, `#444`, `#0e0e0e` around lines
      110-112 et al.) replaced with the matching palette
      variables (`--panel-bg`, `--track-bg`,
      `--panel-border`, ...) so a palette tweak needs one
      edit.

JS:

- [ ] `browserPanel.onOpenChange` (`browser.js:805`)
      single-slot callback either converted to a small
      listener list or given a comment stating last-caller-
      wins and that `presetBar` is the sole subscriber —
      silent drop of a second subscriber is the trap.
- [ ] `panels.js` send-wrapper monkey-patch block
      (`panels.js:72-79`) gains a comment noting wrappers
      must capture-and-delegate `orig` so multiple patchers
      compose.

Rust:

- [ ] `descriptor_to_json` (`src/lib.rs:339`)
      `.expect("json object")` removed (build the map
      directly or `as_object_mut` with infallible-path
      comment). Cosmetic — `json!({...})` is statically an
      object — but it is the pattern 0115 is purging.
- [ ] Decide the local `descriptor_to_json` /
      `taper_to_json` vs the near-identical pair in
      `vxn-core-ui-web/src/lib.rs:645`: delegate to the
      shared versions or comment why the local copy stays.
- [ ] `#[allow(dead_code)] view_event_to_json`
      (`src/lib.rs:497`): move under `#[cfg(test)]` or drop
      the allow if actually used.

Global:

- [ ] Vitest suite (incl. 0016 additions) green; Rust
      `cargo test -p vxn-ui-web` green with `VXN_JS_TESTS=1`;
      faceplate visually unchanged (manual open in a host —
      CSS variable swap is the only render-touching change).

## Notes

The vitest opt-in gate (`VXN_JS_TESTS=1`) stays as-is — CI
(0116) sets it, which removes the original risk of the
suite silently never running.
