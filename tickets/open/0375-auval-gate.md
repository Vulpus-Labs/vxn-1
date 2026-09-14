---
id: "0375"
product: vxn-2
title: "auval green for VXN2, and gated in CI"
priority: high
created: 2026-09-10
epic: E051
depends: ["0374"]
---

## Summary

`auval` is Apple's AU conformance tool and it is considerably stricter than any
host we currently ship into — parameter ranges and defaults, scope and element
handling, state save/restore round-trips, render sanity across sample rates and
buffer sizes, initialisation/uninitialisation cycles. A `.component` that loads
in Logic can still fail it. This ticket is where the first-pass failures get
fixed, and where the gate goes in so they cannot come back. Ninth ticket of
[E051](../../epics/open/E051-audio-unit-distribution.md).

Budget real time here: this is the ticket in the epic most likely to surface
work that is not about AU at all, but about what the parameter model or state
serialisation does at the edges.

## Acceptance criteria

- [ ] `auval -v aumu Vxn2 Vlps` passes locally on a `--universal` build.
- [ ] The bundle CI macOS leg runs it and fails the job on a non-zero exit,
      alongside the existing force-load assertion
      ([bundle.yml:57-60](../../.github/workflows/bundle.yml#L57-L60)).
- [ ] Any parameter-model or state changes needed to pass are made in
      `vxn2-clap` / `vxn2-app` rather than papered over in the wrapper, and each
      is noted in the close-out with what `auval` complained about.
- [ ] State round-trip specifically: save, reinstantiate, restore, and confirm
      the model matches — `auval`'s check and ours should agree.

## Notes

- Dev loop: the AudioComponentRegistrar caches scan results, so a rebuilt
  component can be invisible or stale. `killall -9 AudioComponentRegistrar` and
  clearing `~/Library/Caches/AudioUnitCache` is the reset. Worth putting in the
  README next to the plugin-scan notes rather than rediscovering it.
- `auval` failures are frequently about the *first* instantiation happening
  before `initialize`, which is a different call order from what CLAP hosts use;
  expect the interesting bugs there.
- [[macos-plugin-codesigning]] — an unsigned bundle fails `auval` before any of
  this is reached, so confirm 0374's signing first if the tool refuses to open
  the component at all.
