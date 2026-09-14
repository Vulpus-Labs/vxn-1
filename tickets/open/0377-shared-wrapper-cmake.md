---
id: "0377"
product: monorepo
title: "One shared wrapper CMake module — the link flags should exist once, not four times"
priority: high
created: 2026-09-10
epic: E051
depends: ["0376"]
---

## Summary

`vxn-2/wrapper/CMakeLists.txt` and `vxn-1b/wrapper/CMakeLists.txt` are the same
file with different names — 182 diff lines, all identifiers and comments. Every
link fix has had to be made in both, and the ones that were not caught in both
shipped: the `/WHOLEARCHIVE` + `/OPT:REF` combination produced a ~516 KB module
with an empty factory in VXN1, VXN2 and VXN1b 0.0.1, and the link *succeeded*
each time
([vxn-2/wrapper/CMakeLists.txt:127-156](../../vxn-2/wrapper/CMakeLists.txt#L127-L156),
war story in [0315](../closed/0315-vxn1b-war-story-sweep.md)).

[E010](../../epics/open/E010-vst3-via-clap-wrapper.md) asked for a shared
wrapper up front and it never happened. [0378](0378-vxn3-wrapper-project.md) and
[0379](0379-vxn4-wrapper-project.md) would take the copy count from two to four,
with AU doubling the surface again — so this is the moment. Eleventh ticket of
[E051](../../epics/open/E051-audio-unit-distribution.md).

## Design

`cmake/vxn_wrapper.cmake` at repo root, providing one function:

```cmake
vxn_add_wrapper_targets(
  PRODUCT      vxn2                       # staticlib stem, bundle id suffix
  DISPLAY_NAME VXN2                       # bundle stem
  BUNDLE_ID    labs.vulpus.vxn2
  FORMATS      vst3 auv2                  # what this product ships
  AU_SUBTYPE   Vxn2                       # ignored unless auv2 requested
  AU_MANUFACTURER_CODE Vlps
  AU_MANUFACTURER_NAME "Vulpus Labs"
)
```

Everything currently duplicated moves inside: deployment target, C++ standard,
MSVC runtime policy, OBJC/OBJCXX enablement, the `add_subdirectory` of
clap-wrapper, the per-platform force-load block, the native dependency lists,
and the POST_BUILD staging. Each product's `wrapper/CMakeLists.txt` becomes the
`project()` call plus one function call plus its own input validation.

The comments carrying the war stories move with the code they explain — they are
the reason the block is correct, and a shared module is where they finally stop
being duplicated too.

## Acceptance criteria

- [ ] `cmake/vxn_wrapper.cmake` exists; vxn-2's and vxn-1b's wrapper files are
      reduced to `project()` + inputs + one `vxn_add_wrapper_targets` call.
- [ ] `VXN2.vst3`, `VXN2.component`, `VXN1b.vst3`, `VXN1b.component` all build
      byte-comparably to before (same size class, same force-load assertion,
      same `auval` result) on macOS.
- [ ] Windows: `VXN2.vst3` and `VXN1b.vst3` still build, still contain their
      bundle id, and the `/INCLUDE:clap_entry` + `/FORCE:MULTIPLE` +
      `/WHOLEARCHIVE` trio is present exactly once in the shared module.
- [ ] The manual standalone-invocation instructions in each wrapper header are
      updated to match the new inputs.
- [ ] No behaviour change lands in this ticket — it is a pure move, verified by
      building all four artifacts before and after.

## Notes

Deliberately sequenced after [0376](0376-vxn1b-au-component.md) rather than
before 0373: refactoring a file while still learning what the AU target needs
would mean designing the abstraction against one known case and one guess. Two
working AU builds is the right input.
