---
id: "0372"
product: monorepo
title: "ADR: AU component identity — the four-character codes, frozen"
priority: high
created: 2026-09-10
epic: E051
depends: []
---

## Summary

An Audio Unit is identified by a `(type, subtype, manufacturer)` triple of
four-character codes baked into the component's `Info.plist`. Hosts key saved
projects on that triple: change a code after release and every session using the
plugin loses it, with no migration path. It is the one decision in
[E051](../../epics/open/E051-audio-unit-distribution.md) that cannot be revised
later, so it gets written down before anything ships. Sixth ticket of E051.

## Design

Proposed, as root `adrs/0004-au-component-identity.md`:

| | code | note |
|---|---|---|
| type | `aumu` | music device (instrument). All four synths are instruments. |
| manufacturer | `Vlps` | Vulpus Labs, matching the `labs.vulpus.*` bundle ids. |
| subtype — vxn-1b | `Vx1b` | |
| subtype — vxn-2 | `Vxn2` | |
| subtype — vxn-3 | `Vxn3` | |
| subtype — vxn-4 | `Vxn4` | |

Bundle identifiers `labs.vulpus.<product>.auv2`, matching the existing
`labs.vulpus.<product>.vst3` convention
([vxn-2/wrapper/CMakeLists.txt:118](../../vxn-2/wrapper/CMakeLists.txt#L118)).

Two constraints the ADR should record as *reasons*, not just rules:

- A manufacturer code of all-lowercase ASCII is reserved by Apple. `Vlps` is
  safe; `vlps` would not be.
- The manufacturer code is shared across all four products and the subtype
  distinguishes them — that is the convention hosts and `auval` expect, and it
  is why the manufacturer code must be settled once for the range rather than
  per product.

## Acceptance criteria

- [ ] `adrs/0004-au-component-identity.md` exists, in the house ADR form
      (Context / Decision / Consequences), recording the table above.
- [ ] It states explicitly that the codes are frozen at first release and why
      (host project recall), and what the migration cost would be if they ever
      changed.
- [ ] The codes live in exactly one place in the build — named here, consumed by
      [0373](0373-vxn2-au-component.md) onwards — not repeated per wrapper.
- [ ] Root `adrs/` index or README updated if one exists.

## Notes

`aumu` is also clap-wrapper's default when `AUV2_INSTRUMENT_TYPE` is unset
([wrap_auv2.cmake:181](../../vendor/clap-wrapper/cmake/wrap_auv2.cmake#L181)),
but relying on a warning-and-default for an unchangeable identifier is the wrong
shape — set it explicitly.

If vxn-4 ever ships an FX variant it would need `aufx` and its own subtype; the
ADR should say so rather than leaving the next person to guess.
