---
id: "0380"
product: monorepo
title: "AU in bundle CI and the release — artifacts, auval gates, install docs"
priority: medium
created: 2026-09-10
epic: E051
depends: ["0375", "0376", "0378", "0379"]
---

## Summary

Ship it: AU legs in `bundle.yml`, `.component` artifacts in the release, and
install instructions that name the Components directory. Last ticket of
[E051](../../epics/open/E051-audio-unit-distribution.md).

`bundle.yml` today runs `--format clap,vst3` on macOS and Windows per product,
with a force-load assertion and an artifact upload each
([bundle.yml:53-73](../../.github/workflows/bundle.yml#L53-L73)). AU is a third
format on the macOS legs only.

## Acceptance criteria

- [ ] Every macOS bundle leg runs `--format clap,vst3,au --universal` and
      uploads `<PRODUCT>-macOS-universal-au` alongside the existing artifacts,
      for all four products.
- [ ] Each macOS leg asserts the force-load (bundle id present in the
      `.component` binary) and runs `auval -v aumu <subtype> Vlps`, failing the
      job on either.
- [ ] Windows legs are untouched — same formats, same assertions, same
      artifacts.
- [ ] `release.yml` packages the `.component` bundles into the release archives
      with the same naming convention as the `.vst3` ones.
- [ ] `RELEASING.md` covers the AU artifacts; the install docs name
      `~/Library/Audio/Plug-Ins/Components` and mention the
      AudioComponentRegistrar cache reset from
      [0375](0375-auval-gate.md).
- [ ] A dry-run release produces a set of archives containing CLAP, VST3 and AU
      for every product that ships one.

## Notes

Four products × three formats is a lot of near-identical YAML; `bundle.yml` is
already 307 lines of it. If a matrix falls out naturally while adding the AU
legs, take it — but not at the cost of losing the per-product assertions, which
are the part that has actually caught bugs.

Release notes for the version that carries this should say plainly which hosts
were verified (Logic, Live) rather than implying every AU host was tested.
