---
id: "0320"
product: vxn-1b
title: "Test gaps the review exposed: untested path-escape guard, tests that exercise copies"
priority: medium
created: 2026-08-26
epic: E047
depends: ["0321"]
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
- [ ] All suites green, 0 skipped, under [[0321]].

## Notes

- Item 1 is worth doing even if the rest of [[E047]] is deferred. The web build
  takes preset names from the browser and hands them to a guard nothing tests.
- Item 3's widget is small, but it is also the only faceplate JS that has never
  been linted — treat whatever the first lint pass reports as part of this
  ticket rather than as a surprise.

## Close-out (2026-08-28)

All four items. Six commits. Two of them turned out to be bug fixes rather than
coverage, which is the argument for the ticket.

### 1. `preset_io`'s filesystem half

Nothing here touched the filesystem: the two existing tests covered the embedded
factory bank and stopped. The path-escape guard — reused by
[`vxn1b-web-controller`'s user store](../../vxn-1b/crates/vxn1b-web-controller/src/user_store.rs),
where the names arrive from a browser — had no test at all.

Testing it must not depend on, or write into, the developer's real preset
directory, so the base became an argument, per the ticket's own principle:
[`ensure_within(base, target)`](../../vxn-1b/crates/vxn1b-engine/src/preset_io.rs)
is the guard, `ensure_within_user_dir` resolves the base and delegates, and each
user-preset operation gained a `*_in(base, ..)` inner. The tests drive the
shipping logic.

Nine tests. The guard ones assert it **refusing**:

- `..` traversal, bare and buried mid-path (`Bass/../../outside.toml` — the
  buried one would pass a string-prefix check, which is why the guard
  canonicalises)
- an absolute path elsewhere, and `/etc/passwd`
- a symlink planted *inside* the tree pointing out of it
- delete / move / rename each refusing an outside path, with the file still
  present afterwards — refused, not deleted

Plus `sanitize_name` (separators and traversal characters, the trim, the
never-empty fallback), `unique_folder_name` (case-insensitive count-up, gap
filling), and `tempdir` round-trips for the folder and preset operations
including uniquify-on-collision and refuse-to-clobber.

**Verified non-vacuous:** with the guard's check removed, exactly the three
guard tests fail.

One correction to the ticket's implied expectation: `sanitize_name("///")` is
`"___"`, not `"Untitled"` — every separator maps to `_`, so the result is
non-empty and still a single safe segment. Only an empty-after-trim name falls
back. The test says so.

`tempfile` is a new dev-dependency. The `[dev-dependencies]` section held only a
stale comment for the VXN1 render-parity oracle, which left the workspace when
vxn-1 was archived on 2026-08-27.

### 2. The hydrate opcodes, tested through the code that ships

Fixed the shape, not the test: the bodies moved onto `ControllerState` as
`hydrate_folder_arg` / `hydrate_preset_arg`, the `extern "C"` functions are
one-line shims, and `vxnc_hydrate_folder_on` / `vxnc_hydrate_preset_on` are
gone.

`hydrate_clamps_lengths_that_overrun_the_staged_buffer` then covers what no test
reached: an overrunning record length, an overrunning key length, and an
overrunning folder name. **Verified non-vacuous** — restoring the stand-in's
unclamped slicing fails it with `range end index 2147483659 out of range for
slice of length 12`, which is the panic the clamp prevents.

### 3. `WEB_BOOT_HEAD` is assets now, and the tempo control has a suite

[`assets/web-boot.css`](../../vxn-1b/crates/vxn1b-ui-web/assets/web-boot.css) and
[`assets/web-boot-bpm.js`](../../vxn-1b/crates/vxn1b-ui-web/assets/web-boot-bpm.js),
`include_str!`'d like every other asset, so the Vitest glob reaches them. What
stays inline is the transport shim: it carries the double-underscore
substitution tokens and so cannot be a valid `.js` file — a template, not a
program.

13 tests: the clamp at both ends, the non-finite rejection, the chrome row's
idempotent mount whichever of it and the CPU meter is first, the seed-on-mount,
the write-back of the clamped value, and the keydown stop.

**The first test pass found a real bug**, fixed rather than pinned. `Number('')`
is `0`, not `NaN`, so the old `isFinite` check let an empty field through and
clamped it to 20 — and `<input type=number>` reads back as `''` whenever its
contents are invalid. **Clearing the box, or typing a letter, slammed the tempo
to 20 BPM.** Blank is now rejected outright.

**And a bug introduced writing it, now guarded.** The splice is a blind textual
replace over the whole page, so a substitution token in a spliced *asset* is
filled in there too. A comment in `web-boot-bpm.js` naming the params token had
the entire 25 KB descriptor JSON pasted into it — which still parsed as a
comment, so nothing broke visibly. Caught by diffing the assembled page.
`no_asset_contains_a_placeholder_token` now fails on that, exempting the three
assets that are templates by design (`faceplate.html`, `bridge.js`, the shim).

The assembled web page is otherwise unchanged — diffed ignoring comments, the
only differences are the intended restructure and the blank-field fix.

### 4. Widget factories with suites of their own

[`discrete-switches.test.js`](../../vxn-1b/crates/vxn1b-ui-web/assets/__tests__/discrete-switches.test.js)
— 12 tests for `makeSwitch` (bool and enum) and `makeHeaderSwitch`. `makeWave`
got [`wave.test.js`](../../vxn-1b/crates/vxn1b-ui-web/assets/__tests__/wave.test.js)
in [[0318]]; `makeDropdown` died in [[0310]].

The property worth pinning is that both are **echo-driven**: a click posts a
`discrete` opcode and paints nothing, so the widget stays dark until the
engine's `ParamChanged` returns. A widget that painted locally on click would
look identical until the engine refused the value — which is exactly when it
matters — so the bool cases assert that clicking twice posts `1` twice, and the
state only flips after an echo lands. Also: enum rows are radios not toggles,
exactly one lights per echo, out-of-range echoes clamp, the `>= 0.5` threshold,
the markup label winning over the descriptor label, and the header switch's hit
area being the whole cell rather than the lamp.

The stubs in `dispatch-orchestration.test.js` **stay**: that suite legitimately
tests orchestration — that the right factory is called per cell kind — and
stubbing is the correct technique there. The ticket's complaint was that the
widgets appeared *only* as stubs, which is no longer true.

### Suites ([[0321]]'s four commands)

- `cargo test --workspace` — **1402 pass, 0 fail** (was 1393). VXN1b's own
  share: **420 pass, 0 fail, 0 ignored**. The 5 workspace-wide `ignored` are
  pre-existing `#[ignore]`s in vxn-2 (long sweeps, a diagnostic) and one
  `vxn_core_utils` doc-test — none in VXN1b.
- vitest — **343 pass** (was 318), **0 skipped**
- `xtask web` — clean
- `node --test` — **158 pass, 0 skipped, 0 todo**

### One criterion read narrowly, deliberately

> `ensure_within_user_dir` has tests covering `../` traversal, an absolute path,
> and a symlink **if that is representable on both CI platforms**.

The symlink test is `#[cfg(unix)]`. Creating one on Windows needs either
Developer Mode or elevation, so a cross-platform version would be a test that
silently no-ops on half the matrix. It runs on the macOS job.
