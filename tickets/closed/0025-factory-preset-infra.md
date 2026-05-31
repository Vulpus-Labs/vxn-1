---
id: "0025"
title: Factory bank infrastructure + starter set
priority: medium
created: 2026-05-28
epic: E007
---

## Summary

Stand up the factory-preset bank: a source tree of `.toml` presets embedded into
the binary at compile time, an enumeration API the browser consumes, and a CI
test that every shipped factory preset parses and round-trips. Seed it with a
small set of **VXN1-original** presets (the JP-8 port is a separate payload,
0028) so the pipeline is exercised end-to-end. Decisions:
[ADR 0005](../../adrs/0005-vxn1-presets.md) §4.

## Acceptance criteria

- [x] Source tree `crates/vxn-engine/presets/factory/<category>/<name>.toml`
  (category = directory; e.g. `Bass/`, `Pad/`, `Lead/`, `Keys/`).
- [x] Embed via `include_dir!` (add `include_dir` dep). A `factory()` API yields
  the embedded presets — at minimum `{ relative_path, category, parsed }`, with
  parse deferred or eager (eager is fine; the bank is small).
- [x] A `FactoryPreset` listing API the browser (0027) can call without touching
  the filesystem: enumerate by category, fetch by id/path, get `meta.name`.
- [x] **CI test**: iterate every embedded factory file, `from_toml_str` it,
  assert no parse error and **zero warnings** (factory files must be clean — no
  unknown keys, no bad enum labels), and that `kind`/`schema` are current.
- [x] A starter set of ~6–8 VXN1-original presets across a few categories,
  covering each `kind` (at least one `performance` exercising Dual or Split) and
  exercising the breadth of the param model (sync/FM, ring, noise, both LFOs,
  unison/twin, key-track).
- [x] `cargo xtask bundle` unaffected (presets are in the binary, not the
  bundle) — confirm the build still produces a loadable `.clap`. (Bundle builds;
  note: the embedded `FACTORY` static is LTO-stripped from the cdylib until 0027
  references `factory()` — see the doc note in `factory.rs`.)

## Notes

- **Embed, don't bundle** (ADR 0005 §4): `include_dir!` over the source tree
  means no install step, nothing to lose at runtime, and the CI round-trip test
  validates the bank at build time. Staging into `Contents/Resources` for the
  future preset-discovery route (ADR 0005 §7) is explicitly out of scope here.
- Author the starter presets *sparse* and *by ear* against the current defaults;
  they double as worked examples of the format for 0028's author.
- Keep category names stable and human — they are the browser's grouping and
  show up in the UI verbatim.
- Don't gold-plate the listing API; the browser only needs "list by category"
  and "load this one". `factory()` returning a flat slice the UI groups is fine.
