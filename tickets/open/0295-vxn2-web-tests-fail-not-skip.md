---
id: "0295"
product: vxn-2
title: "vxn-2 web suite hides its own integration failures behind skips"
priority: high
created: 2026-08-25
epic: null
depends: []
---

## Summary

`node --test vxn-2/crates/vxn2-wasm/web/*.test.mjs` reports **89 pass** on a tree
where 11 of those tests cannot run. They are guarded `{ skip: !HAVE }` on an
artifact path, and when the artifact is absent they are silently skipped — the
run is green, the summary line says `skipped 13`, and nothing draws attention to
the fact that every test which touches the real wasm just opted out.

That is not hypothetical. It is how [0285](0285-web-param-mirror-drift.md) hid:
vxn-2's browser build could not boot for weeks, and the 11 tests that would have
caught it on the first run were skipped in every run anybody did.

Two things combine:

1. **Skip instead of fail.** A missing artifact is a setup problem with a known
   fix (`build it`), not a reason to declare the test inapplicable. Skipping
   turns "I could not check this" into a green tick.
2. **The guarded path is shared.** Four files look for the controller wasm under
   `target/web-dist/`, which **both** ports' `xtask web` create *and wipe*
   (`remove_dir_all` then rebuild). So running vxn-1's `xtask web` disarms
   vxn-2's integration coverage, at a distance, with no signal.

vxn-1's equivalents already fail loudly and read from their own target directory;
VXN1b's ([0287](0287-vxn1b-sab-transport-js.md)) were written this way
deliberately. vxn-2 is the odd one out.

## Design

- **Fail, never skip.** Every wasm-backed test asserts its artifact exists, with
  the exact build command in the message. A test that cannot verify its subject
  must say so in a way that stops the run.
- **Read the crate's own artifact.** The controller wasm lives at
  `target/wasm32-unknown-unknown/{release,debug}/vxn2_web_controller.wasm` — the
  crate's own build output, which no other product's tooling touches. Only
  `controller-wasm.test.mjs` genuinely needs the *bundle*, because it asserts
  against the real baked `factory.bin`; it keeps that dependency and names the
  command that produces it.

### Not doing: separate per-product dist directories

The obvious root-cause fix is to stop the two ports sharing `target/web-dist`.
Deliberately not doing it here: that path is baked into both `deploy-web.sh`
scripts, both `serve-coep.mjs` defaults, `WEB-HOSTING.md`, and the READMEs — it is
the live publishing flow ([[vxn-web-publish-flow]]), and changing it to fix a test
visibility problem trades a small, contained bug for a chance of breaking
deploys. Once the tests no longer depend on the shared directory, the collision
is benign: overwriting a bundle is what asking for a bundle means. Worth doing on
its own terms if the ports ever need to be served side by side, which is a
different ticket with a different risk profile.

## Acceptance criteria

- [ ] No `skip:` remains in `vxn-2/crates/vxn2-wasm/web/*.test.mjs`.
- [ ] Each wasm-backed test fails with a message naming the build command.
- [ ] The controller-wasm tests read `target/wasm32-unknown-unknown/**`, not
      `target/web-dist/`.
- [ ] `controller-wasm.test.mjs` still uses the real baked bank, and says what to
      run when it is missing or belongs to another product.
- [ ] `node --test vxn-2/crates/vxn2-wasm/web/*.test.mjs` reports **89 pass,
      0 skipped** on a built tree.
- [ ] Verified by hand that removing the artifact turns the run red, not green.

## Notes

- Found while building VXN1b's transport; the same guard shape was deliberately
  avoided there.
- vxn-1's suite already behaves correctly — no change needed.
