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
