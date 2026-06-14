---
id: "0009"
product: vxn-1
title: Vendor clap-wrapper + vst3sdk 3.8 as submodules
priority: high
created: 2026-06-08
epic: E010
---

## Summary

Add `vendor/clap-wrapper` (free-audio/clap-wrapper) and
`vendor/vst3sdk` (steinbergmedia/vst3sdk at the 3.8 line) as
pinned git submodules. Neither enters the Cargo graph; both
are consumed by the wrapper CMake (ticket 0010).

Per ADR 0008 §1.

## Acceptance criteria

- [ ] `.gitmodules` registers:
      - `vendor/clap-wrapper` → free-audio/clap-wrapper, at a
        named release tag (latest stable as of the ticket
        date).
      - `vendor/vst3sdk` → steinbergmedia/vst3sdk, at the
        latest 3.8.x tag.
- [ ] `git submodule update --init --recursive` from a fresh
      clone leaves both checkouts at the pinned commits.
- [ ] License audit: top-level `LICENSE` (or `LICENSE.txt`)
      in each submodule confirms MIT. Note the audit result
      in the commit message.
- [ ] README adds a "Submodules" subsection under "Building"
      explaining `git submodule update --init --recursive`
      is required before `cargo xtask bundle --format vst3`.
- [ ] No new files under `vxn-1/`, `crates/`, or the Cargo
      workspace — vendored code lives under `vendor/` at the
      repo root only.

## Notes

clap-wrapper itself bundles a VST3 SDK as its own submodule,
likely pre-3.8 on the chosen tag. The wrapper CMake (ticket
0010) overrides that path to point at our `vendor/vst3sdk`
checkout via cmake variable — do **not** modify the wrapper's
own submodule pointer. Keeping our SDK pin independent of the
wrapper's release cadence means a future wrapper bump doesn't
silently change the SDK version we ship against.

Submodule depth: shallow is tempting (vst3sdk in particular is
large) but `git submodule update --init --depth 1` interacts
badly with later tag bumps. Take the disk hit; full clones
are easier to maintain.

If a contributor builds without initialising submodules, the
CLAP-only path must still work — gate the wrapper CMake call
in xtask (ticket 0011) behind `--format vst3` so a fresh clone
can `cargo xtask bundle --format clap` with no extra setup.

Pinning recommendation at ticket date: pick the most recent
*tagged* commits on each repo's default branch; record the
exact SHAs in the commit message so the pin reason is
recoverable if the tags later move.
