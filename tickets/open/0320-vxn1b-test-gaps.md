---
id: "0320"
product: vxn-1b
title: "Test gaps the review exposed: untested path-escape guard, tests that exercise copies"
priority: medium
created: 2026-08-26
epic: E047
depends: ["0309"]
---

## Summary

Four coverage gaps, found by reading what the tests actually reach rather than
by counting them. VXN1b's test suites are strong in aggregate — the routing
table, ring-before-store ordering, boot queue splicing, the DOM modal and the
gesture gate are all properly covered — which is why these specific holes are
worth naming.

### 1. `preset_io`'s entire filesystem half is untested — including the escape guard

The only two tests in
[preset_io.rs:538-576](../../vxn-1b/crates/vxn1b-engine/src/preset_io.rs#L538-L576)
cover the factory bank. Nothing exercises
[`ensure_within_user_dir`](../../vxn-1b/crates/vxn1b-engine/src/preset_io.rs#L124)
— the path-escape guard — nor `sanitize_name`, `unique_folder_name`,
`rename_user_preset`, `move_user_preset`, or the folder operations. All of them
are re-exported and **reused by
[`vxn1b-web-controller/src/user_store.rs`](../../vxn-1b/crates/vxn1b-web-controller/src/user_store.rs)**,
where user-supplied preset names arrive from a browser.

`sanitize_name` and `unique_folder_name` are pure and trivially testable; the
escape guard wants a `tempdir` case with `../` and an absolute path.

This is the highest-value item here — it is the only one where the untested code
is a security-shaped guard rather than a convenience.

### 2. Two opcodes are tested via copies that drop the production clamping

[web-controller/src/lib.rs:1873-1888](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L1873-L1888)
defines `vxnc_hydrate_folder_on` / `vxnc_hydrate_preset_on`, which
**re-implement** `vxnc_hydrate_folder` ([:1047](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L1047))
and `vxnc_hydrate_preset` ([:1059](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L1059))
because the real ones read the global `STATE`.

The copies drop the clamping: the shipped `vxnc_hydrate_preset` clamps
`start`/`end` against `arg_in.len()`
([:1062-1065](../../vxn-1b/crates/vxn1b-web-controller/src/lib.rs#L1062-L1065)),
the test stand-in slices raw and would panic. So **the only defensive code in
that opcode is the part no test reaches.**

Fix the shape, not the test: extract the bodies onto `ControllerState` as
`hydrate_folder_arg(&mut self, len)` / `hydrate_preset_arg(...)`, make the
`extern "C"` shims one line, and point the tests at the methods.

### 3. `WEB_BOOT_HEAD` is 96 lines of CSS+JS inside a Rust string literal

[vxn1b-ui-web/src/lib.rs:368-463](../../vxn-1b/crates/vxn1b-ui-web/src/lib.rs#L368-L463)
carries the web-only BPM widget as inline source, while every other asset in the
crate is `include_str!`'d from `assets/`. Consequence:
[`vitest.config.js`](../../vxn-1b/crates/vxn1b-ui-web/assets/vitest.config.js)
globs `__tests__/**/*.test.js` against `assets/*.js`, so this widget — which
clamps and posts `set_tempo` — is **the one piece of faceplate JS with no test
and no lint**.

Move it to `assets/web-boot.html` (or `.css` + `.js`) and `include_str!` it,
matching `BRIDGE_JS` / `FACEPLATE_CSS`. Then give it a test.

### 4. Widget factories with no suite of their own

`makeSwitch`, `makeHeaderSwitch`, `makeWave` and `makeDropdown` appear only as
**stub factories** in
[dispatch-orchestration.test.js:70](../../vxn-1b/crates/vxn1b-ui-web/assets/__tests__/dispatch-orchestration.test.js#L70)
— so the orchestration suite proves they are *called*, not that they work.
`makeDial`, `makeRocker`, `makeBipolar`, `makeMeter`, `makeScope`,
`matrixOverlay` and `presetBar` are all properly covered by comparison.
(`makeDropdown` dies in [[0310]]; the other three need suites.)

### Related, from elsewhere in the epic

- **`EventRing._push` is validated only against a decoder that does not ship** —
  [[0312]] owns this; noted here so the gap is recorded in one place.
- **`meterEvent`/`scopeEvent` are not bound to the Rust serialiser** — [[0316]].
- **The matrix enum tables' tests check lengths, not correspondence** — [[0319]].

## Design

Nothing subtle. Two principles:

1. **Where a test exercises a copy, change the code so it can exercise the
   original.** Item 2 is the case in point — the stand-in exists because the
   production function reads a global, which is a testability defect in the
   production function, not in the test.
2. **Where the untested thing is a guard, test the guard failing**, not just the
   guard passing. `ensure_within_user_dir` returning `Ok` for a good path proves
   very little.

## Acceptance criteria

- [ ] `ensure_within_user_dir` has tests covering `../` traversal, an absolute
      path, and a symlink if that is representable on both CI platforms.
- [ ] `sanitize_name` and `unique_folder_name` have unit tests.
- [ ] The user-preset folder operations have at least a `tempdir` round-trip.
- [ ] `vxnc_hydrate_folder` / `vxnc_hydrate_preset` are tested through the code
      that ships, clamping included; the `_on` duplicates are gone.
- [ ] `WEB_BOOT_HEAD` is an `include_str!`'d asset, covered by the Vitest glob,
      with a test for the tempo clamp.
- [ ] `makeSwitch`, `makeHeaderSwitch` and `makeWave` have real suites, not
      stubs.
- [ ] All suites green, 0 skipped, under [[0309]].

## Notes

- Item 1 is worth doing even if the rest of [[E047]] is deferred. The web build
  takes preset names from the browser and hands them to a guard nothing tests.
- Item 3's widget is small, but it is also the only faceplate JS that has never
  been linted — treat whatever the first lint pass reports as part of this
  ticket rather than as a surprise.
