---
name: release
description: Cut a new VXN release — pick the version, write the notes, run `cargo release`, deploy the web builds and repoint the site. Use when the user says "cut a release", "ship a release", "new release", "release 0.4.0", or "/release".
---

# Cut a VXN release

VXN1b and vxn-2 ship **together**: one version, one bare-semver tag, one release page, eight assets, plus both browser builds and the two product pages on `vulpuslabs.com`.

The mechanics are already automated — [`cargo release`](../../../RELEASING.md), source in [`xtask/`](../../../xtask). **Do not reimplement them by hand.** This skill is the judgement around the tool: choosing the version, writing the notes, and deciding what to do when a gate refuses.

Read [RELEASING.md](../../../RELEASING.md) first if you have not. It carries the step table, the wasm/rustup trap, and per-step failure recovery.

## Steps

1. **Check `main` is green before anything else.** `gh run list --branch main --limit 3`. A red `main` is a **blocker to fix and land first**, not something to release around — at 0.3.0 the Test job had been failing since the previous commit and the release would have shipped a browser build with a dead control. If it is red, stop, diagnose, fix, land, and only then come back. Say so plainly rather than proceeding.

2. **Survey what is in the release.** `git log --oneline <last-tag>..HEAD`, plus tickets and epics closed in that window (`git diff --name-status <last-tag>..HEAD -- tickets/closed epics/closed`). You need this for both the version decision and the notes; read the close-out sections, they are written for exactly this.

3. **Agree the version with the user.** `MAJOR.MINOR.PATCH`, digits only — no pre-release suffix, the workflow triggers on the bare tag. Minor for user-visible feature work, patch for fixes only. This is the user's call, not yours: ask with the actual content of the release in front of them (what changed, whether patches still sound the same), and recommend one.

4. **Run the free checks early.** `cargo release preflight` then `cargo release verify`. Together these take several minutes and catch the things that are cheap now and expensive after a tag is public. `verify` runs **both** ports' `node --test` web suites — vxn-2's are not in `test.yml`, so this is the only thing that runs them.

   A failure here is a real bug, not a flaky gate. Fix it, land it, and restart from step 1. Do not release around it.

5. **Write the release notes** to `release-notes/<version>.md`. `publish` refuses without them. Shape that has worked (see `release-notes/0.3.0.md` and the 0.2.0 release page):

   - a one-paragraph lede naming the release's theme;
   - **preset compatibility** — say explicitly whether saved patches still sound the same, and why. This is the first thing a player wants to know;
   - **what they will actually hear** — a re-baselined golden in a ticket close-out is the signal;
   - a section per product, then the web builds, then downloads with the macOS de-quarantine commands.

   Write for players, not for the commit log. "The scale VCA gets the same nine curves" beats "0341 landed".

6. **Run `cargo release all <version>`.** It does preflight → verify → bump → publish → web → site → check, skipping what step 4 already completed, and stops at every irreversible action to confirm.

   **Never pass `--yes` unless the user explicitly asks for an unattended run.** Those prompts are the user's decision points: pushing to `main`, publishing a public release page, and deploying the live site. Surface each one and let them answer.

7. **Report honestly.** Name the tag, the asset count, the two live product pages, and anything you could not verify. The browser hand-check — pick a control in each web build, confirm it changes the sound and survives a preset load — **cannot be automated and is not covered by `check`**. Say so rather than implying the release is fully verified.

## Notes

- **A step failed mid-release?** `all` records progress in `target/vxn-release/ledger`, keyed by version. Re-run the same command to resume; completed steps are skipped. RELEASING.md's "Recovering from a failed step" covers each case, including the important one: if `publish` failed *after* the release page was created, do **not** delete the release — re-run the build job and re-run `publish` to re-check the assets.
- **Do not run the individual steps by hand** (`gh release create`, `deploy-web.sh`, editing site front matter) when the tool would do it. Every gate in it exists because that failure has actually happened.
- **The site is a second repository** (`~/src/vulpus-labs-site`, `SITE=` overrides). `preflight` pulls it — it was four commits behind when 0.3.0 was cut. Netlify deploys from `main`, so pushing it *is* the deploy.
- **`release` in the site front matter is the git tag**, not display text — download URLs resolve against it. Both product pages carry `version` and `release` and both move together; the tool errors if either is missing.
- **Adding a product to the release?** vxn-3 and vxn-4 are workspace members that do not ship. A new shipping product needs jobs in [`release.yml`](../../../.github/workflows/release.yml), entries in `EXPECTED_ASSETS` in [`xtask/src/plan.rs`](../../../xtask/src/plan.rs), a `deploy-web.sh`, and a product page. The asset manifest is deliberately hard-coded so a job dropping out of the workflow fails the release instead of quietly shrinking it.
- Commits the tool makes follow the repo convention already; if you commit anything alongside, use the land-on-main skill's format.
