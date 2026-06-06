---
id: "0091"
title: Modal mount-target audit
priority: low
created: 2026-06-01
epic: E017
---

## Summary

The 0075 audit's finding N7: `mountModal` in
[browser.js:556](../../crates/vxn-ui-web/assets/browser.js#L556)
hard-mounts every modal under `document.getElementById('faceplate')`,
and the modal helpers (`mountModal`, `openConfirmModal`,
`openSaveAsModal`) live inside `browserPanel`'s closure. Any
non-browser code that wanted a modal confirm (a key-mode rename?
a bad-preset toast that needs acknowledgement?) couldn't reach
the helpers.

Audit, don't refactor prematurely. If E016 / E017 surfaced a
second use case for the modal, lift `mountModal` into
`browser/modal.js` (or `controller/modal.js`) and parameterise
the mount target. If no second use case has emerged, document the
lift trigger in a comment and leave the code where it is.

## Acceptance criteria

- [ ] Walk the four E016 tickets (0081–0085) and the five E017
      tickets (0086–0090) at close-out. Report in this ticket's
      Notes section:
      - Did any of them want a modal confirm? (E016/0081's
        dead-code removal — no. E017/0088's controller factory
        — no, the existing browser modals satisfy. Others — fill
        in.)
      - Did any other panel grow a confirm need outside the
        browser? (Today: no.)
- [ ] **If yes (at least one second use case):**
      Lift `mountModal` into `controller/modal.js` (or a new
      `assets/modal.js` if the 0090 reorg has different folder
      conventions). It takes `{ title, danger, okLabel, onOk,
      mountInto }` where `mountInto` defaults to
      `document.getElementById('faceplate')`. `openConfirmModal`
      and `openSaveAsModal` move with it. `browserPanel`
      imports them and passes its own mount target if needed.
      Add a test in
      `__tests__/modal.test.js` covering: open/close lifecycle,
      ESC dismisses, backdrop click dismisses, OK invokes
      `onOk` exactly once, double-click on OK doesn't fire
      twice.
- [ ] **If no:** Add a comment block above `mountModal` (and a
      note in the 0090-derived `browser/modal.js` if that file
      exists, otherwise at the IIFE site):
      ```
      // mountModal currently hard-mounts under #faceplate and
      // is only invoked from this file. Lift to a standalone
      // helper if a second caller appears (e.g. a non-browser
      // panel needing a confirm). See 0091.
      ```
- [ ] Manual smoke (ask first): browser delete-confirm and
      Save-As modals still work; ESC and backdrop click still
      dismiss; OK still invokes the right action.
- [ ] `npm test` and `cargo test -p vxn-ui-web` pass.

## Notes

This ticket is deliberately conditional — premature extraction
of `mountModal` would create a new helper with no second user,
which is the same kind of speculative abstraction the audit
flagged elsewhere. The trigger is "a second concrete caller",
not "this *might* be useful one day".

Close-out comment must include the audit result either way so
the decision is recorded. If the answer is "no", the next person
who needs a modal sees the comment and knows the lift is a
small, pre-scoped move.
