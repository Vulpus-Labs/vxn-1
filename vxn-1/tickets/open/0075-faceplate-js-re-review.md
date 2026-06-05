---
id: "0075"
title: Re-review faceplate JS against the original findings
priority: high
created: 2026-06-01
epic: E014
---

## Summary

After 0063–0074 close, re-walk
[bridge.js](../../crates/vxn-ui-web/assets/bridge.js),
[browser.js](../../crates/vxn-ui-web/assets/browser.js) (new in 0073),
[panels.js](../../crates/vxn-ui-web/assets/panels.js), and
[dispatch.js](../../crates/vxn-ui-web/assets/dispatch.js).
Confirm every 2026-06-01 review finding is addressed and that the
cleanup didn't introduce new instances of the same pattern. This is
the closing audit on E014, not new design work.

## Acceptance criteria

- [ ] **Per-finding sweep.** For each of the 16 original findings,
      confirm in writing (one line per finding in the close-out
      comment) that it is resolved:

      | # | Finding | Verifier |
      |---|---------|----------|
      | 1 | Gesture-bracketed writes (13 sites) | grep `beginGesture` — only `makeFader` + `makeDetuneLegato`'s fader drag remain |
      | 2 | Fader drag scaffolding duplicated | only `wireFaderDrag` callers — no inline drag handler chains |
      | 3 | Hover/drag valuePop lifecycle | only `attachValuePop` callers — no inline `valuePop.show / update / hide` outside the helper |
      | 4 | Variant clamp 4× | grep `Math.max(0, Math.min(variants.length` returns 0 hits outside `clampVariant` |
      | 5 | Pointer-norm 2× | grep `1 - (ev.clientY - r.top) / r.height` returns 0 hits outside `wireFaderDrag` |
      | 6 | tgRow miss in keysPanel | grep `ctl-tg-box` in keysPanel returns only via `tgRow()` |
      | 7 | Opcode strings scattered | grep `op: '` returns 0 hits — only typed sender names |
      | 8 | Variant-by-name lookups | grep `variants.indexOf(` returns 0 hits outside `variantIdx` |
      | 9 | Dim rules split between dispatch + DIM_RULES | dispatch has one `applyDimRulesFor(ev.id, ev.plain)` branch — no `ev.id === FREE_RUN_ID` / `ev.id === FILTER_MODE_ID` |
      | 10 | LAYERED_CELLS + STATIC_CELLS | one `model.cells` array; entries carry `layered: bool` |
      | 11 | Dispatch state as globals | grep `const ` and `let ` at module-level in `dispatch.js` returns only function defs + `model` decl |
      | 12 | browserPanel ~770 lines in panels.js | `browser.js` exists; `panels.js` does not contain `const browserPanel` |
      | 13 | openModal body-polymorphism + extendActions | grep `extendActions` returns 0 hits; `openConfirmModal` + `openSaveAsModal` distinct |
      | 14 | Magic numbers (3000ms, 120ms) | `STATUS_PILL_FLASH_MS` + `KNOB_INDICATOR_TRANSITION_MS` defined at module scope |
      | 15 | TWIN_TOP_CT buried | `TWIN_TOP_CT` defined at module scope alongside `PIXELS_PER_DETENT` |
      | 16 | (wave glyphs / KEY_MODE_NAMES already good) | no change; confirm still good |
- [ ] **No regression sweep.** Walk the three (now four) files end
      to end looking for *new* instances of the same patterns. Specific
      things to look for:
      - Bypass of typed senders — any new code path that calls
        `window.ipc.postMessage` directly or constructs a `{op: …}`
        object outside `bridge.js`.
      - Bypass of `discrete` — any new `beginGesture / setParam /
        endGesture` triplet outside `bridge.js`'s `discrete` helper.
      - State leaks — any new module-level `let` / `const` in
        `dispatch.js` (or `browser.js`) outside the `model` object.
      - Helper bypass — bare `Math.max(0, Math.min(variants.length …`
        / `variants.indexOf('…')` / `1 - (ev.clientY - r.top) / r.height`
        anywhere. Should be 0 hits in each grep.
      - Magic numbers — any new numeric literal in a `setTimeout`,
        a CSS transition string, or a clamp ceiling that isn't
        named at module scope.
- [ ] **Fresh review pass.** Re-read each file in full, looking
      for *new* findings that didn't surface in the 2026-06-01
      review — patterns that emerged from the cleanup, or that
      were obscured by the original mess. Document any genuine
      finding (don't manufacture them) as either:
      - A follow-up ticket under E014 (if it fits the cleanup
        epic).
      - A new note in the close-out comment, with a recommendation
        for a future epic if E014 is the wrong home.
- [ ] **Smoke.** Per the `ask-before-screen-capture` rule, ask
      first: run the plugin in a host. Confirm:
      - Every panel renders.
      - Every fader drags + commits via gestures (host records as
        one edit, not many).
      - Every wave knob rotates + glyph-click selects directly.
      - Every switch / buttongroup / dropdown / header-switch
        flips state.
      - Detune-legato: Twin clamps detune to 20 ct on entering
        Twin; Legato dims outside Mono modes.
      - Layer flip rebinds all per-patch panels.
      - Key-mode flip shows/hides the split-row + edit-toggle
        correctly.
      - Preset bar prev/next/Browse/Save As all work; status pill
        flashes on load warnings.
      - Browser panel: search, click-load, context menu rename /
        delete / move, "+ New" folder, DnD, modal confirms.
      - Text-input popup commits and cancels.
- [ ] `cargo test -p vxn-ui-web` passes — the full substring
      suite, including any new assertions added by the per-finding
      tickets.

## Notes

This is a written audit, not a refactor pass. If the audit
surfaces a real new finding the author *can* fix it inline
(small, obvious cleanups) — but anything bigger goes to a
follow-up ticket so the audit stays an audit.

The grep checks listed above are sanity probes, not exhaustive
proofs. The substantive part of this ticket is the close-out
comment that explicitly names each of the 16 findings and where
the resolution lives (which sibling ticket, which line in which
file). That comment is what future-you reads when wondering
whether E014 actually landed what it said it would.

If a finding turned out to be a non-issue once attempted — e.g.
the `attachValuePop` helper read worse than the inline form once
written — record that too. "We tried and rolled back because X"
is a valid resolution and worth preserving.

## Close-out audit (2026-06-01)

### Per-finding sweep

All sixteen 2026-06-01 findings are resolved. Greps below run from
[crates/vxn-ui-web/assets/](../../crates/vxn-ui-web/assets/).

| #  | Finding                                | Verifier result |
| -- | -------------------------------------- | --------------- |
| 1 | Gesture-bracketed writes | `grep beginGesture` shows three sites — `makeFader` ([panels.js:375](../../crates/vxn-ui-web/assets/panels.js#L375)), `makeWave` ([panels.js:574](../../crates/vxn-ui-web/assets/panels.js#L574)), `makeDetuneLegato` ([panels.js:860](../../crates/vxn-ui-web/assets/panels.js#L860)). Every one brackets a *drag* (continuous norm/plain writes during pointermove), which is genuinely not a `discrete()` shape. Verifier checklist forgot the wave-knob drag — it was always there, not new. All click-to-set sites go through `send.discrete(…)`. |
| 2 | Fader drag scaffolding | `setPointerCapture` appears twice — once each in `wireFaderDrag` ([panels.js:285](../../crates/vxn-ui-web/assets/panels.js#L285)) and `makeWave` ([panels.js:573](../../crates/vxn-ui-web/assets/panels.js#L573)). The wave knob does *not* go through `wireFaderDrag` — it has its own pointerdown / move / up triplet. Defensible (wave drag is rotational not linear-norm; the wired callbacks would be a different shape) but worth flagging — see new finding N2 below. |
| 3 | Hover/drag valuePop lifecycle | Five `valuePop.X` calls remain, all inside the `attachValuePop` factory ([panels.js:314–335](../../crates/vxn-ui-web/assets/panels.js#L314-L335)). Every caller (`makeFader`, `makeWave`, `makeDetuneLegato`) goes through `pop.markX` / `pop.refresh`. ✓ |
| 4 | Variant clamp | One hit: `clampVariant` itself ([panels.js:633](../../crates/vxn-ui-web/assets/panels.js#L633)). ✓ |
| 5 | Pointer-norm | One hit: inside `wireFaderDrag` ([panels.js:270](../../crates/vxn-ui-web/assets/panels.js#L270)). ✓ |
| 6 | tgRow miss in keysPanel | `keysPanel` uses `tgRow()` at [panels.js:112](../../crates/vxn-ui-web/assets/panels.js#L112) and [panels.js:132](../../crates/vxn-ui-web/assets/panels.js#L132). Remaining inline `ctl-tg-box` literal is inside `makeDetuneLegato` ([panels.js:816](../../crates/vxn-ui-web/assets/panels.js#L816)) — a tg-row stamped as part of a composite cell rather than a standalone row. Not a miss, but borderline (see N3). |
| 7 | Opcode strings scattered | `op: '` appears only in `bridge.js` ([35–62](../../crates/vxn-ui-web/assets/bridge.js#L35-L62)). ✓ |
| 8 | Variant-by-name lookups | `variants.indexOf(` appears once, inside `variantIdx` ([dispatch.js:32](../../crates/vxn-ui-web/assets/dispatch.js#L32)). ✓ |
| 9 | Dim rules split | One `applyDimRulesFor` branch ([dispatch.js:357](../../crates/vxn-ui-web/assets/dispatch.js#L357)). No `ev.id === FREE_RUN_ID` / `FILTER_MODE_ID` survives. ✓ |
| 10 | LAYERED_CELLS + STATIC_CELLS | `model.cells.push({…, layered: isLayeredEl(el)})` ([dispatch.js:317–324](../../crates/vxn-ui-web/assets/dispatch.js#L317-L324)); one iteration in `rebindAllForLayer`. ✓ |
| 11 | Dispatch state as globals | Module-level decls in `dispatch.js` are `const model = {…}` and `const BUILTIN_DIM_SPECS = […]` plus function defs — no free `let`. ✓ |
| 12 | browserPanel out of panels.js | `browserPanel` references in `panels.js` are all *callers*; the IIFE itself lives in `browser.js`. ✓ |
| 13 | openModal polymorphism | No `extendActions` anywhere; `openConfirmModal` ([browser.js:606](../../crates/vxn-ui-web/assets/browser.js#L606)) and `openSaveAsModal` ([browser.js:625](../../crates/vxn-ui-web/assets/browser.js#L625)) are distinct, sharing only `mountModal`. ✓ |
| 14 | Magic numbers | `STATUS_PILL_FLASH_MS` ([bridge.js:93](../../crates/vxn-ui-web/assets/bridge.js#L93)), `KNOB_INDICATOR_TRANSITION_MS` ([panels.js:245](../../crates/vxn-ui-web/assets/panels.js#L245)). ✓ |
| 15 | TWIN_TOP_CT buried | `TWIN_TOP_CT` ([panels.js:252](../../crates/vxn-ui-web/assets/panels.js#L252)) alongside `PIXELS_PER_DETENT` ([panels.js:240](../../crates/vxn-ui-web/assets/panels.js#L240)). ✓ |
| 16 | Wave glyphs / KEY_MODE_NAMES | Unchanged. ✓ |

### No-regression sweep

- No `window.ipc.postMessage` outside `_post` in `bridge.js`. ✓
- No `{op: …}` literal outside `bridge.js`. ✓
- No `setTimeout(…, <literal ms>)` outside the `STATUS_PILL_FLASH_MS` site. ✓
- No `Math.max(0, Math.min(variants.length …)` outside `clampVariant`. ✓
- No `variants.indexOf(` outside `variantIdx`. ✓
- No bare `1 - (ev.clientY - r.top) / r.height` outside `wireFaderDrag`. ✓
- Module-level state in `browser.js` is a single IIFE binding (`const browserPanel`); inside the IIFE, state is namespaced to that closure. ✓
- Module-level state in `panels.js`: `presetBar`, `keysPanel`, `WAVE_GLYPHS`, constants block, `SVG_NS`. All justified domain pegs / IIFE singletons. ✓

### Fresh findings (post-cleanup)

These were either obscured by the original mess or emerged as side
effects of the refactor. None are bugs. Each is sized for a
follow-up ticket (or punted to a larger reuse epic — see below).

**N1 — Primitive factories couple directly to `window.vxn.send`.**
Every `makeX` reaches into the global typed sender (e.g.
[panels.js:375](../../crates/vxn-ui-web/assets/panels.js#L375),
[:512](../../crates/vxn-ui-web/assets/panels.js#L512),
[:725](../../crates/vxn-ui-web/assets/panels.js#L725)). Fine for one
plugin; blocks reuse from vxn-2 and blocks unit-testing the primitives
in isolation. The bridge is already the right abstraction — primitives
just need to receive it as an argument rather than read the global.

**N2 — `makeWave` re-implements drag scaffolding.** `wireFaderDrag`
captures the linear-norm protocol; the wave knob's vertical-pixel-
delta drag is structurally similar (down → capture, move → update,
up/cancel → release, gesture brackets, hover/drag popup wiring) but
parameterised differently (pixel delta + clamp instead of [0, 1] norm).
A second helper `wireKnobDrag` — or a generalised `wireDrag` taking
a `pointerToValue(ev, start)` function — would collapse
[panels.js:566–595](../../crates/vxn-ui-web/assets/panels.js#L566-L595)
down to a callback set, and the `hovered`/`dragging` locals would go
through the same getter shape `attachValuePop` already expects.

**N3 — `makeDetuneLegato` open-codes a tg-row.** Inline at
[panels.js:815–817](../../crates/vxn-ui-web/assets/panels.js#L815-L817).
`tgRow()` returns a fresh `<div>` chain; the composite needs it
mounted under the existing `.ctl-detune-legato` container with a
fixed "LEGATO" label. Either parameterise `tgRow` to accept a target
element (or return innerHTML), or accept a small drift.

**N4 — `makeFader` and `makeDetuneLegato` duplicate the thumb-from-norm
math.** `setThumb` ([panels.js:351–364](../../crates/vxn-ui-web/assets/panels.js#L351-L364))
and `setThumbFromPlain` ([panels.js:839–846](../../crates/vxn-ui-web/assets/panels.js#L839-L846))
are the same `halfThumb + (1 - n) * travel` formula. The detune
version additionally maps plain-to-norm via the Twin-aware ceiling.
A `paintFader(fader, thumb, norm)` helper would absorb the first;
the composite then becomes `paintFader(fader, thumb, plain / currentTop())`.

**N5 — `paramIdByName` is O(n) per call, hit per cell during rebind.**
[dispatch.js:6–11](../../crates/vxn-ui-web/assets/dispatch.js#L6-L11)
walks every entry in `window.vxn.params` linearly. Today's param count
(~150) per layer-rebind (~50 cells) is invisible perf-wise, but a
name → id index built once in `init()` keeps the linearity off the
hot path and makes the code read as "look up", not "scan".

**N6 — `model.cells.push` mixes "I saw this DOM node" with binding
state.** Today `cells` is the source of truth for *what to rebind on a
layer flip*. If a panel becomes hidden via CSS or replaced (a future
e.g. "preset detail" overlay), `model.cells` keeps a stale reference.
Not exercised today; mark for revisit if any panel goes
dynamically-mounted.

**N7 — Modal mount target is `getElementById('faceplate')`.** Browser
panel modals can't be opened by callers outside the browser panel.
Save-As works because it's a `browserPanel` method invoked from
`presetBar`. If anything else (key-mode rename? bad-preset toast?)
needs a confirm modal it can't reuse `mountModal`. Lift `mountModal`
to a small standalone `modal.js` helper *only if* a second site
appears — premature otherwise.

**N8 — `_browserOpen` global side channel.** [browser.js:385](../../crates/vxn-ui-web/assets/browser.js#L385)
sets `window.vxn._browserOpen` but nothing reads it. Dead code left
over from the pre-`onOpenChange` design. Delete.

**N9 — `appendMenuItem`'s `renameLabel` ternary is identical on both
sides.** [browser.js:429](../../crates/vxn-ui-web/assets/browser.js#L429):
`target.kind === 'preset' ? target.name : target.name`. Cleanup leftover
— same expression both branches; reduces to `target.name`.

### Reuse strategy (for VXN-2 forward-port)

VXN-2 will share sliders, selection buttons, rotary dials, and MVC.
The current shape is *close enough* to lift wholesale, but four
boundaries are crossed today that future-VXN2 will have to undo:

1. **Bridge** — `window.vxn.send` is a typed sender already. Make it
   the *injected* dependency, not the global. Concretely: each
   primitive factory takes `(el, id, desc, { send, opts })`. The
   global keeps existing for the bootstrap loader; primitive code
   reads `send`, not `window.vxn.send`.
2. **Param descriptors** — `window.vxn.params` is currently the only
   model. Wrap it: `params.get(id) → desc`, `params.idByName(name)`,
   `params.idByNameAtLayer(name, layer)`. Build the reverse index
   once at `init()` (addresses N5).
3. **Controller** — promote `model` + `bindCell` + `rebindAllForLayer`
   + `applyDimRulesFor` into a single `createController({ params, send,
   primitives })` factory. Each `make*` is registered against its
   `data-control` kind, then `bindCell` dispatches on the lookup
   table instead of a `switch`. This is the M and C of the MVC the
   user named.
4. **Primitives = the V.** Each `make*` becomes a pure factory:
   - Input: `el`, `id`, `desc`, `{ send, displayOverride? }`.
   - Output: `{ update(plain, norm, display), dispose() }`.
   - Side effects: only on `el`. No `addCtl` from inside the
     factory — the controller does that.
   - Today's factories are 80% of the way there: they take `(el, id,
     desc, opts?)` and return `{ update }`. The missing 20% is the
     `window.vxn.send` coupling (addressed by item 1) and `addCtl`
     coupling (currently *only* `makeDetuneLegato` calls `addCtl`
     — every other primitive just returns `ctl` and `bindCell` does
     the registering; lift the composite's three calls back into
     `bindCell` and the boundary is clean).

Concrete vxn-2 lift list, in order of size:
- `valuePop`, `statusPill`, `wireFaderDrag`, `attachValuePop`,
  `clampVariant`, `tgRow`, `glyphPath`, `WAVE_GLYPHS`, the
  constants block — drop-in.
- `makeSwitch`, `makeButtonGroup`, `makeDropdown`, `makeHeaderSwitch`,
  `makeFader`, `makeWave` — drop-in after item 1 above.
- `makeDetuneLegato` — composite, may not survive into vxn-2 unchanged
  (Twin mode is a vxn-1 voice-stealing detail; ADR-0002 lists Twin
  among the deliberate v1 simplifications).
- `keysPanel` — drop-in after generalising the IIFE into a factory
  that takes `{ ipc, parent, defaults }`.
- `browserPanel` — drop-in. Already self-contained; only host-global
  reads are `window.vxn.send`, `window.vxn.promptText`, and the
  faceplate root id.
- Bridge + dispatch.js init wiring — vxn-2-specific; rewrite per
  that plugin's param table.

This is **not** an E014 deliverable. It's a forward note for a
future E0XX "JS reusable component library" epic when vxn-2 starts.
Filing it here so future-you doesn't have to re-derive it from a
clean read of the four files.

### Unit-testing methodology (recommended)

The substring suite in `vxn-ui-web/src/lib.rs` catches gross
regressions (typed senders exist, opcode strings match Rust's
`UiEvent`, status-pill markup present) but verifies *no behaviour*.
With the four files now at ~155 / 445 / 780 / 925 lines, behavioural
coverage is the next correctness lever.

Recommended path, in three increments:

**Phase 1 — bring the JS files to ESM source, keep splice-loading at
build time.** Today each file is concatenated by `lib.rs::build_faceplate_html`
into a single `<script>`. If each file adopts `export`s for its
public surface (`bridge.js` → `{ send, params, subdivisions, promptText,
statusPill, valuePop }`; `panels.js` → `{ makeFader, makeWave, …,
keysPanel, presetBar }`; `browser.js` → `{ browserPanel }`;
`dispatch.js` → `{ init }`), then:
   - The wry-side `<script>` block adds `type="module"` *or* a
     trivial strip-`^export `-prefix pass in the splice loader keeps
     the legacy concat behaviour. Pick the latter to avoid touching
     wry's eval path.
   - Source files become Node-`import`-able verbatim.

**Phase 2 — Vitest + jsdom for the primitives.** `npm init -y` inside
`crates/vxn-ui-web/assets/`, add `vitest` + `jsdom` as
`devDependencies` (no production dependency on Node). Test layout:
```
crates/vxn-ui-web/assets/
├── bridge.js
├── panels.js
├── browser.js
├── dispatch.js
├── package.json                  // dev-only
├── vitest.config.js
└── __tests__/
    ├── fader.test.js
    ├── wave.test.js
    ├── button-group.test.js
    ├── browser-move-targets.test.js
    ├── dim-rules.test.js
    └── keys-panel.test.js
```
A primitive test reads:
```js
import { makeFader } from '../panels.js';
import { JSDOM } from 'jsdom';

const dom = new JSDOM('<div data-control="fader"></div>');
const el = dom.window.document.querySelector('[data-control="fader"]');
const sends = [];
const send = { setParamNorm: (id, n) => sends.push(['n', id, n]),
               beginGesture: id => sends.push(['b', id]),
               endGesture:   id => sends.push(['e', id]) };
const ctl = makeFader(el, 42, { label: 'Cutoff', max: 1 }, { send });
ctl.update(0.5, 0.5, '500 Hz');
// dispatch a pointerdown / pointermove / pointerup, assert sends shape.
```

**Phase 3 — wire into `cargo test`.** A `#[test]` in `lib.rs` shells
`npm test --silent` under `cfg(not(miri))` and asserts exit code 0.
Wrapped in `#[ignore]` by default if Node isn't available locally;
CI removes the ignore. Alternative: keep the JS suite as a separate
GH Actions job and gate PR merge on it independently. The first
keeps `cargo test -p vxn-ui-web` honest; the second is lighter.

Recommended initial coverage:
- `clampVariant`, `variantIdx`, `glyphPath`, `keysNoteName`,
  `paramIdByNameAtLayer`, `subdivisionLabel`, `moveTargets`,
  `folderOptions`, `folderValue` — pure functions, no DOM. Fast and
  load-bearing.
- `wireFaderDrag` callback ordering (enter / down / move / up / leave
  / cancel + hover-during-drag) — needs jsdom pointer events.
- `attachValuePop` lifecycle (hover-only, drag-only, hover-during-
  drag, drag-ends-outside) — DOM but mockable.
- `browserPanel.followPath` (selects + scrolls), `setCorpus` /
  `renderPresets` invariants (selected folder survives a corpus
  refresh, dead folder collapses to root).
- Dim rule resolution against a fixture `params` table — assert the
  built `model.dimRules` matches the spec for both layers.

Boundary that *won't* test from Node:
- wry's IPC actual delivery (Rust-side).
- WKWebView quirks (drag-on-Safari, native popup menu, OS native
  text-input window). Manual smoke covers these.
- CSS layout. The substring suite is the existing pin; the JS suite
  doesn't extend it.

**Cost estimate.** Phase 1 is ~half a day (one strip-pass + ~20
`export` statements). Phase 2 is ~1–2 days to land Vitest + jsdom +
the first 6 tests above. Phase 3 is ~half a day for the cargo-side
shim. Total ~2.5–3 days for the first behavioural net.

**Sequence with the reuse strategy.** Phase 1 is a prerequisite for
the vxn-2 lift (you can't `import` a non-module). So *if* the JS
component library epic is on the roadmap, phase 1 lands first
regardless. Phase 2 is its first deliverable — a regression net
that survives the move to vxn-2.

### Recommendations summary

- E014 closes cleanly on the 16-finding axis. ✓
- Fresh findings N1, N2, N4 are small enough to fold into a single
  follow-up ticket (e.g. "JS primitive boundary cleanup") if the
  reuse epic is queued; otherwise leave as-is.
- N8 (dead `_browserOpen`) and N9 (`renameLabel` ternary) are
  one-line fixes — author can land inline per the ticket's "small
  obvious cleanups" allowance.
- N3, N5, N6, N7 are notes for the reuse epic, not bugs.
- The unit-test methodology is the highest-leverage forward
  investment. Recommend filing **E015 — JS unit-test net** with the
  three-phase plan above as its scope.
- Manual smoke still required per the ticket — not yet performed.
