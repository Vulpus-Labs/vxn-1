# Releasing VXN

**One release line.** vxn-2 and VXN1b share a version, a bare-semver tag and a
release page, and every published release builds both products: four jobs, eight
assets. It was not always so — until 0.2.0 the two shipped on separate tag lines
(vxn-2 on the bare semver, VXN1b on `vxn-1b-<version>`), and
[`release.yml`](.github/workflows/release.yml)'s header records why that split
was retired. Nothing reads the tag name any more.

A release touches **two repositories**: this one, and
[`vulpus-labs-site`](https://github.com/Vulpus-Labs/vulpus-labs-site), whose
product pages carry the download links and host the browser builds. The site
auto-deploys from `main` via Netlify, so **pushing the site is the deploy.**

## The short version

```bash
# write the notes first — publish refuses without them
$EDITOR release-notes/0.3.0.md

cargo release all 0.3.0
```

`all` runs the seven steps below in order, stopping at each irreversible action
to confirm. It records progress in `target/vxn-release/ledger` and skips what is
already done, so a failure in step 6 does not cost the ten minutes of GitHub
waiting in step 4. Re-run the same command to resume.

Run `cargo release --help` for the flags. `--yes` skips the confirmations for an
unattended run; `SITE=<path>` points at a different site checkout.

## The steps

Each is independently runnable (`cargo release verify`, `cargo release site
0.3.0`, …) and independently re-runnable. Run them one at a time when something
has gone wrong, or when you want to stage a release across a break.

| # | Step | What it does | Reversible? |
|---|---|---|---|
| 1 | `preflight` | Tools present and `gh` authenticated; wasm32 target installed; this repo on `main`, clean, synced with origin; **site checkout clean and pulled**; vxn-1b's JS deps installed | yes |
| 2 | `verify` | Both web bundles built, `cargo test --workspace` with `VXN_JS_TESTS=1`, **both** ports' `node --test` web suites, `cargo bench --no-run` | yes |
| 3 | `bump` | `[workspace.package] version`, `Cargo.lock` refreshed, committed | local commit |
| 4 | `publish` | Push `main` → wait for all three workflows on that exact commit → tag and create the release → wait for the four build jobs → assert all eight assets attached | **no** |
| 5 | `web` | Rebuild both browser bundles and mirror them into the site checkout (commits there, does not push) | local commit |
| 6 | `site` | Repoint both product pages' `version` **and** `release` front matter, then push the site | **no — this is the deploy** |
| 7 | `check` | From outside: all eight asset URLs, COOP/COEP on both web pages, both product pages linking the new tag | yes |

### Why the order is load-bearing

Nothing is pushed before it is verified. Nothing is tagged before CI agrees on
that specific commit. The site is not repointed at a release whose assets have
not attached — a product page pointing at a tag with no binaries is a 404 on the
download button, and the site is the only place most people meet these plugins.

## Things that have actually gone wrong

Each of these is now a gate rather than a habit.

- **The site checkout was four commits behind origin** when 0.3.0 was cut.
  Running `deploy-web.sh` against a stale checkout either pushes a merge or
  fails after the release is already public. `preflight` pulls it.
- **CI was red on `main` before the release even started.** The workspace test
  job had been failing since the previous commit; the release would have shipped
  a browser build with a dead control. `publish` refuses to tag unless all three
  workflows are green on the exact commit being tagged, and treats *no runs yet*
  as pending rather than as success — GitHub takes a moment to register a push,
  and reading that gap as green tags an untested commit.
- **vxn-2's `web/*.test.mjs` suites are not in `test.yml`.** vxn-1b's identical
  suite is. That gap is the whole difference between 0345 being caught the same
  afternoon in one synth and shipping silently in the other. Until the workflow
  grows that leg, `cargo release verify` is the only thing that runs it — which
  is why it is not optional there. **Adding it to `test.yml` is the outstanding
  follow-up**; it needs `cargo xtask web` to build vxn-2's two wasm artifacts
  first, the same ordering constraint 0321 solved for vxn-1b.
- **VXN1b's product page kept `release = "vxn-1b-0.2.0"`** long after the two
  products merged onto one tag. `release` is the *git tag* download URLs resolve
  against, so a stale value serves the previous release's binaries under the new
  version's heading. Both keys move together, and the rewrite errors if either is
  missing rather than passing silently.
- **A VST3 whose Rust staticlib was never force-loaded still links** and still
  produces a plausible bundle with an empty factory. That shipped three times
  before `release.yml` grew its non-hollow check. `publish` additionally asserts
  the asset *count*, so a job dropping out of the workflow fails the release
  instead of quietly shrinking it.
- **The `_headers` COOP/COEP block is appended, never rewritten.** Three synth
  subpaths share that file; overwriting it takes the others off the air, and
  their pages then load and fail to construct a `SharedArrayBuffer` — which
  reads as "the synth is broken", not as "a header is missing". The
  `deploy-web.sh` scripts own this, which is why step 5 delegates to them rather
  than reimplementing the copy. `check` verifies the headers on both pages.

## The wasm toolchain trap

Building any crate for `wasm32-unknown-unknown` needs the **rustup** toolchain,
not a Homebrew rust. Both can be installed, and brew's `rustc` wins on `PATH`;
cargo shells out to a bare `rustc` resolved through `PATH`, so
`--target wasm32-unknown-unknown` fails with

```
error[E0463]: can't find crate for `std`
```

*even after* `rustup target add wasm32-unknown-unknown` — because the target was
added to a toolchain cargo is not using.

`cargo release` fixes this itself for the steps that need it, resolving the
toolchain through `rustup which rustc` so it honours
[`rust-toolchain.toml`](rust-toolchain.toml) rather than hard-coding a triple.
To do it by hand:

```bash
export RUSTC="$(rustup which rustc)"
export PATH="$(dirname "$RUSTC"):$PATH"
```

## Doing it by hand

Every step is a few commands. Reach for these when the tool is in the way, or to
understand what it is doing.

```bash
# 1. preflight
git switch main && git pull --ff-only && git status --porcelain   # must be empty
(cd ~/src/vulpus-labs-site && git pull --ff-only && git status --porcelain)
rustup target list --installed | grep wasm32-unknown-unknown

# 2. verify  (with the toolchain exports above)
cargo run -p vxn1b-xtask -- web
(cd vxn-2 && cargo xtask web)
VXN_JS_TESTS=1 cargo test --workspace
node --test vxn-1b/crates/vxn1b-wasm/web/*.test.mjs
node --test vxn-2/crates/vxn2-wasm/web/*.test.mjs
cargo bench --no-run --workspace

# 3. bump — edit [workspace.package] version in Cargo.toml, then
cargo check --workspace
git add Cargo.toml Cargo.lock && git commit -m "chore(release): 0.3.0"

# 4. publish
git push origin main
gh run list --branch main --limit 3            # wait for all three to be green
gh release create 0.3.0 --title "VXN 0.3.0" \
    --notes-file release-notes/0.3.0.md --latest
gh run list --workflow=release.yml --limit 1   # wait for the four build jobs
gh release view 0.3.0 --json assets --jq '.assets[].name' | wc -l   # must be 8

# 5. web
NO_PUSH=1 ./vxn-1b/crates/vxn1b-wasm/deploy-web.sh
NO_PUSH=1 ./vxn-2/crates/vxn2-wasm/deploy-web.sh

# 6. site — set version AND release to the tag in both pages, then
cd ~/src/vulpus-labs-site
$EDITOR content/products/vxn-1b/index.md content/products/vxn-2/index.md
hugo --quiet                                   # optional local render check
git add content/products/vxn-1b/index.md content/products/vxn-2/index.md
git commit -m "Point both product pages at the 0.3.0 release" && git push origin main

# 7. check
cargo release check 0.3.0
```

## Recovering from a failed step

- **`publish` failed while waiting on CI.** Nothing was tagged. Fix the code,
  commit, and re-run `cargo release publish <version>` — it pushes the new
  commit and waits again.
- **`publish` failed *after* the release was created.** The tag and the release
  page exist; only the binaries are missing or short. Do not delete the release —
  re-run the build with `gh run rerun <id>`, then `cargo release publish
  <version>` again to re-check the assets. It sees the existing release and skips
  creation.
- **`site` failed before the push.** The front-matter commit is sitting in the
  site checkout. Re-run `cargo release site <version>`; it commits nothing new
  and goes straight to the push.
- **`check` failed on an asset.** The release page is public with a broken
  download. Re-run the release build job; the site links resolve as soon as the
  asset attaches, with no site change needed.
- **The ledger is wrong** (a step recorded as done that you want to redo). It is
  a plain text file, one `<version> <step>` per line: delete the line, or delete
  `target/vxn-release/ledger` to start the run over. `cargo clean` also clears it.

## Release notes

`publish` reads `release-notes/<version>.md` (override with `--notes`) and
refuses to run without it, because `gh release create` will happily publish an
empty release page.

The shape that has worked: a one-paragraph lede naming the release's theme, a
note on **preset compatibility** if anything in the patch format moved, then a
section per product, then the web builds, then downloads with the macOS
de-quarantine commands. [0.2.0](https://github.com/Vulpus-Labs/vxn-1/releases/tag/0.2.0)
and [0.3.0](https://github.com/Vulpus-Labs/vxn-1/releases/tag/0.3.0) are the
worked examples.

Two things worth saying explicitly every time, because they are what players
actually need to know: **whether their saved patches still sound the same**, and
**which of the changes they will hear**. A re-baselined golden in a ticket
close-out is the signal for the second one.

## Versioning

`MAJOR.MINOR.PATCH`, digits only — the release workflow triggers on the bare
semver tag, and `cargo release` rejects anything else rather than let you
discover it after the tag is public. Minor for user-visible feature work, patch
for fixes. There is no pre-release channel.

The workspace version is the single source: `[workspace.package] version` in
[`Cargo.toml`](Cargo.toml), inherited by every member, and stamped into each
macOS bundle's `Info.plist` from that product's own `CARGO_PKG_VERSION`.
