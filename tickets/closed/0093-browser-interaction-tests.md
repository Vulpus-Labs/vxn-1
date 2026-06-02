---
id: "0093"
title: Browser-panel and preset-bar behavioural coverage
priority: medium
created: 2026-06-01
epic: E017
depends_on: ["0092"]
---

## Summary

Six user-facing flows in
[browser.js](../../crates/vxn-ui-web/assets/browser.js) and
[panels.js](../../crates/vxn-ui-web/assets/panels.js) have zero
behavioural assertions today: context-menu Rename / Delete /
Move, HTML5 drag-drop preset moves, modal confirm scaffolding,
search input + clear, preset-bar buttons, and the Keys panel
mode / layer / split-point toggles. None of these are on E017's
primitive-lift path (which is where per-ticket test coverage
naturally lands per the
[E015](../../epics/closed/E015-js-test-framework.md) convention),
so they need their own coverage ticket.

Lands after [0092](0092-drag-test-helpers.md) so the new files
can import the shared `installVxn` / `loadBrowserPanel` /
`browserDOM` helpers without re-defining them five times.

## Acceptance criteria

All new files mount a fresh fixture DOM in `beforeEach`, call
`installVxn(opcodes)` for the opcodes they exercise, then
`vi.resetModules()` + dynamic import per the
[browser-invariants](../../crates/vxn-ui-web/assets/__tests__/browser-invariants.test.js)
pattern (now in `_helpers.js`).

- [ ] [crates/vxn-ui-web/assets/__tests__/browser-context-menu.test.js](../../crates/vxn-ui-web/assets/__tests__/browser-context-menu.test.js)
      covers (drives the openMenu path at
      [browser.js:483](../../crates/vxn-ui-web/assets/browser.js#L483)):
      - Right-click a user folder row →
        `contextmenu` opens the menu; Rename prompts via
        `promptText` and lands `['renameFolder', oldName, newName]`
        in `sendCalls`.
      - Rename keeps `sendCalls` empty when the prompt returns
        `null`, an empty string, or the unchanged name.
      - Delete on a folder row opens the confirm modal; the
        modal's OK button sends `['deleteFolder', name]`; cancel
        sends nothing.
      - Right-click a user preset row → Rename sends
        `['renamePreset', path, newName]`; Delete confirm sends
        `['deletePreset', path]`.
      - Move-to submenu on a preset hits one of the user folders
        excluding the current one (assert against
        `moveTargets`'s output) and sends
        `['movePreset', path, targetName]`.
      - Factory folder rows have no contextmenu handler — assert
        the row has no `.browser-menu` ancestor after dispatching
        `contextmenu`.
- [ ] [crates/vxn-ui-web/assets/__tests__/browser-drag-drop.test.js](../../crates/vxn-ui-web/assets/__tests__/browser-drag-drop.test.js)
      covers
      [browser.js:218-239](../../crates/vxn-ui-web/assets/browser.js#L218-L239)
      and
      [wirePresetDragSource](../../crates/vxn-ui-web/assets/browser.js#L363):
      - `dragstart` on a user preset row stamps
        `vxn/preset` on a stubbed `DataTransfer`, sets
        `effectAllowed = 'move'`, adds `.dragging` to the row.
      - `dragover` on a different user folder row calls
        `preventDefault`, sets `dropEffect = 'move'`, adds
        `.drag-over` to the target.
      - `dragover` on the **same** folder the drag started from
        adds `.drag-blocked` and does **not** call
        `preventDefault` (browser default rejects the drop).
      - `dragleave` clears both `.drag-over` and `.drag-blocked`.
      - `drop` on a valid target sends
        `['movePreset', path, folderName]` and clears the
        highlight; drop on the source folder sends nothing.
      - `dragend` clears `dragSourcePath` (next `dragover`
        becomes a no-op) and clears any stuck `.drag-over` /
        `.drag-blocked` across the folder list.
      - jsdom omits `DataTransfer`; stub a minimal recording
        shim (`{ setData, effectAllowed, dropEffect }`) and
        attach to each dispatched `DragEvent` (use
        `new Event('dragstart')` + `Object.defineProperty`,
        same shape as `pointerEvt`).
- [ ] [crates/vxn-ui-web/assets/__tests__/browser-modals.test.js](../../crates/vxn-ui-web/assets/__tests__/browser-modals.test.js)
      covers
      [openModal / closeModal](../../crates/vxn-ui-web/assets/browser.js#L595)
      via the Delete-confirm entry point (the only modal that
      isn't already touched by `browser-invariants`'s Save-As
      tests):
      - Backdrop click closes the modal (`#document.querySelector('.browser-modal')`
        is null after the click).
      - The cancel button closes the modal; no opcode is sent.
      - The OK button sends the bound opcode and closes the
        modal.
      - Re-opening from the menu after a cancel works
        (no stale modal state).
- [ ] [crates/vxn-ui-web/assets/__tests__/browser-search.test.js](../../crates/vxn-ui-web/assets/__tests__/browser-search.test.js)
      covers the search input handler and clear button at
      [browser.js:758](../../crates/vxn-ui-web/assets/browser.js#L758):
      - Typing into `#browser-search-input` and dispatching
        `input` filters `#browser-presets` to rows whose name
        contains the query (case-insensitive); each row carries
        a `.browser-row-origin` span.
      - Empty matches show the "No matches" placeholder.
      - The clear button resets the input value to `''` and
        re-renders the folder-selected preset list.
      - Search hits in user folders carry both a contextmenu
        and `draggable=true`; factory hits carry neither.
- [ ] [crates/vxn-ui-web/assets/__tests__/preset-bar.test.js](../../crates/vxn-ui-web/assets/__tests__/preset-bar.test.js)
      covers
      [panels.js:37-54](../../crates/vxn-ui-web/assets/panels.js#L37-L54).
      The preset bar buttons live in `panels.js`'s
      `initPresetBar` (or whatever the post-0064/0067 name is —
      check at implementation time). Mount the four buttons
      (`#preset-prev`, `#preset-next`, `#preset-browse`,
      `#preset-save`) and a stub for `browserPanel`
      (`setOpen` / `isOpen` / `openSaveAs` recording shims),
      then:
      - Prev click → `['stepPreset', -1]`.
      - Next click → `['stepPreset', 1]`.
      - Browse click when closed → `browserPanel.setOpen(true)`;
        when open → `browserPanel.setOpen(false)`.
      - Save click → `browserPanel.openSaveAs(currentName)`
        with the bar's current preset name.
- [ ] [crates/vxn-ui-web/assets/__tests__/keys-panel.test.js](../../crates/vxn-ui-web/assets/__tests__/keys-panel.test.js)
      covers
      [panels.js:120-184](../../crates/vxn-ui-web/assets/panels.js#L120-L184):
      - Pointerdown on a mode row that isn't the active one
        sends `['setKeyMode', i]`; the active row sends nothing.
      - Pointerdown on an edit-layer row sends
        `['setEditLayer', code]`; active row no-ops.
      - Edit-layer list is `visibility: hidden` in Whole mode,
        `visible` otherwise.
      - Split row is `visibility: visible` in Split mode only.
      - Split-slider `input` event clamps the value to
        `[KEYS_SPLIT_MIN, KEYS_SPLIT_MAX]` and sends
        `['setSplitPoint', clampedNote]`; the readout updates
        optimistically (assert `keysNoteName(value)` is the
        new text).
      - Split-slider `dblclick` sends
        `['setSplitPoint', KEYS_DEFAULT_SPLIT]`.
      - Reset button in Whole sends two `resetLayer` opcodes
        (`'upper'` and `'lower'`); in Dual / Split it sends one
        for the current layer only.
- [ ] All new files use `installVxn` from `_helpers.js` (0092);
      none redefine `browserDOM`, `pointerEvt`, `mountEl`,
      `loadBrowserPanel`, or `installVxn` inline.
- [ ] `npm test` passes; the suite gains at least 30 new
      assertions across the six files.
- [ ] `cargo test -p vxn-ui-web` passes (unchanged).

## Notes

The Keys panel's mode / layer / split rendering is gated on
internal `mode` / `layer` state that isn't reachable from the
returned object's API. To drive the visibility tests, expose a
test-only `setMode(m)` / `setLayer(c)` *or* assert via the
returned `update` setters that already exist
([panels.js:191](../../crates/vxn-ui-web/assets/panels.js#L191)).
Read the file at implementation time — don't add a new public
surface if the existing setters suffice.

Context-menu submenu items: `appendMoveSubmenu` shows them
inside a hidden `.browser-submenu` that CSS reveals on hover.
jsdom doesn't run CSS, but the DOM nodes exist regardless —
just dispatch `click` on the submenu item directly.

`DataTransfer` stub: keep it minimal — `setData(type, value)`
records into a Map, `getData(type)` reads it back,
`effectAllowed` / `dropEffect` are plain assignable strings.
The browser code only writes these; it never inspects the
recording.

The current preset name passed to `openSaveAs` is sourced from
the preset-bar's most recently-rendered name. Inspect
`panels.js` at implementation time for the exact accessor —
the bar may carry a closure variable or a `data-current-name`
attribute. Don't guess.

Some of these surfaces (context menus, modals, search) will
likely be reshaped by E017's
[0073-split-browser-js](../../tickets/open/0073-split-browser-js.md) /
[0074-unbundle-modals](../../tickets/open/0074-unbundle-modals.md)
follow-ons. That's fine — the tests assert *behaviour*, not
internal structure. If the split lands first, the tests get
imported against the new modules with a one-line `from` change.
