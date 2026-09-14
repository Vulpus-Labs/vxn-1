---
id: "0387"
product: vxn-4
title: "vxn4-ui-web and the CLAP gui extension — the faceplate opens in a host"
priority: high
created: 2026-09-10
epic: E052
---

## Summary

Get the prototyped faceplate on screen inside the plugin. New crate
`vxn4-ui-web` bundling the page assets and wrapping
[`vxn-core-ui-web`](../../crates/vxn-core-ui-web)'s wry host, plus the `gui`
extension in `vxn4-clap`, whose description currently ends *"no faceplate"*
([Cargo.toml:8](../../vxn-4/crates/vxn4-clap/Cargo.toml#L8)).

The design is settled in
[`vxn-4/ui-mockup/index.html`](../../vxn-4/ui-mockup/index.html) — three panels,
the control vocabulary, the interaction rules, the popup-readout convention. This
ticket ports it; it does not redesign it. Layout questions get answered by
editing the mockup first, then porting the answer.

Chrome only in this ticket: the page opens, resizes, closes cleanly, and draws.
Binding controls to real values is [0388](0388-vxn4-bind-and-grey.md).

## Acceptance criteria

- [ ] New crate `vxn4-ui-web` with `build_html` over the mockup's markup, CSS
      and JS, split into `assets/` the way vxn-3's crate splits
      `index.html` / `style.css` / `app.js`.
- [ ] `open_editor` over `vxn_core_ui_web::open_editor`, returning the shared
      `EditorHandle` / `OpenEditorError`. A bad parent or a failed wry build
      returns an error; it never panics.
- [ ] `EDITOR_WIDTH` / `EDITOR_HEIGHT` constants matching the mockup's rendered
      size, and the faceplate sized to the **tallest** pane — the operators tab
      is roughly 170px taller than the other two.
- [ ] `gui` extension implemented in `vxn4-clap`: create, set_parent, show,
      hide, destroy, size negotiation.
- [ ] The shared value-popup CSS comes from
      [`vxn-core-ui-web/assets/value-pop.css`](../../crates/vxn-core-ui-web/assets/value-pop.css)
      rather than being copied into vxn-4's stylesheet — the mockup already uses
      that ruleset verbatim.
- [ ] Opens, closes and reopens in a host without leaking a webview or crashing;
      closing with a drag in progress does not panic.
- [ ] Unsigned-bundle and codesigning behaviour unchanged — this ticket must not
      regress plugin scanning.

## Notes

The JS in the mockup is a single inline script that owns its own state, because
that is what a mockup needs. The port has to split that in two: the control
**primitives** (fader, dial, wave picker, toggle, button group, combo, hFader,
meter, the graph canvases) are reusable and belong in a panels-style module set;
the mock state and the fake patch data go away entirely, replaced by the bridge.

E008 (`js-reusable-primitives`) is the open epic covering exactly this
factoring across synths. Check it before writing new primitives — several of
these already exist in vxn-1b's `assets/panels/` and vxn-2's, and the wave knob
in the mockup is already a port of
[vxn-2's knob.js](../../vxn-2/crates/vxn2-ui-web/assets/panels/knob.js). Prefer
extending the shared set to adding a fourth copy.

Two primitives in the mockup are genuinely new and have no precedent to reuse:
the PM grid (with its drag-to-set-depth, double-click-to-toggle and sign
encoding) and the operator wave picker with its drawn preview. Those are vxn-4's
own.

The mockup's `#mixer` / `#operators` / `#matrix` hash routing is a
screenshotting aid for iterating on layout. Keep it — it is how the next round
of design review happens — but it must not be the mechanism the real tab strip
uses.

## Close-out (2026-09-14)

- New crate `vxn4-ui-web`:
  [src/lib.rs](../../vxn-4/crates/vxn4-ui-web/src/lib.rs) with `build_html`,
  `open_editor` over `vxn_core_ui_web::open_editor`, the size constants, and the
  re-exported handle/error types. Assets split into `index.html` / `style.css` /
  `app.js` plus 17 modules under `assets/panels/`, ESM-authored with the export
  syntax stripped at splice and the order declared in one array
  (`html_has_every_asset_spliced`, `the_bundle_carries_no_module_syntax`).
- `gui` extension in [gui.rs](../../vxn-4/crates/vxn4-clap/src/gui.rs); `lib.rs`
  gained only the module, the registration and one handle field. Drives
  create → get_size → destroy twice through `clack-host`
  (`the_host_can_open_and_close_the_editor_twice`); a null parent errors rather
  than panicking (`a_null_parent_is_an_error_and_not_a_panic`).
- **1140 × 803, sized to the tallest pane** — operators is 674px against the
  mixer's 494 and perform's 488. Trimming operators means redesigning three rows
  of real controls; renegotiating `get_size` per tab resizes the host window
  mid-edit. The short panes leave visible space at the bottom; that is the trade.
  `editor_width_matches_the_css`, `editor_height_accounts_for_the_tallest_pane`.
- Reused rather than copied: `valuePop` and its `value-pop.css` (spliced from
  the shared crate — `grep -c '^\.value-pop'` on vxn-4's stylesheet returns 0),
  `wireDrag`, and `noteName`. Genuinely new: the PM grid and the operator wave
  picker, as the ticket predicted. vxn-1b's and vxn-2's fader/dial/button-group
  bind to `data-vxn-param` markup and build no DOM, so they were the shape to
  follow rather than the code to import.
- Four edits to the mockup, made there first and none changing how it renders:
  `--tab-h` / `--pane-h`, an explicit height on `.tab-btn` (its height came out
  of the font's line box), `min-height` on the active pane, and border-box on
  `.op-tab`.
- `cargo xtask bundle` still produces an ad-hoc-signed `vxn4.clap`; no
  `Contents/Resources/` staging, because the assets are `include_str!`-embedded.
- 8 Rust tests in the crate, 19 JS tests behind `VXN_JS_TESTS`
  (`js_suite_passes`), 24 + 12 in `vxn4-clap`. Verified by rendering the real
  assembled page and confirming it matches the mockup, and by driving synthetic
  pointer sequences against the PM grid, the popup and the tab canvases.
- Two things this ticket changed relative to design review: the matrix overlay
  shows **48 rows, not 16**, because it now reads `N_MATRIX_SLOTS` from Rust and
  16 would leave slots 17–48 unreachable; and `ControllerHandle::detached()` was
  added to `vxn-core-app` (additive, documented as transitional) because the
  editor host needs a handle and `vxn4-app` is
  [0386](0386-vxn4-app-crate.md) — a post that fails at the channel beats a stub
  `ParamModel` the page could read wrong values out of.
- Chrome only, as scoped: controls own their own local state and nothing
  dispatches. `set_parent` could not be exercised without a window server; the
  open/close claim rests on the create/destroy cycle test plus `EditorHandle`'s
  drop. Binding is 0388.
- Flagged for E008: the LFO wave-glyph table is now a third copy of vxn-2's
  `panels/knob.js`. Lifting it into core means editing vxn-2, which this ticket
  was scoped out of.
