---
id: "0006"
title: vxn-1 migration onto vxn-core-* + E003 unblock
priority: high
created: 2026-06-06
closed: 2026-06-06
epic: E001
status: split
splits_into: ["0007", "0008", "0009", "0010"]
---

> **Closed as split.** The vxn-dsp re-exports and the bit-identical
> leaf-type dedup in vxn-app shipped under this ticket. The remaining
> scope is split into 0007 (event-Custom rewire — unblocks the rest),
> 0008 (vxn-clap migration), 0009 (vxn-ui-web migration), and 0010
> (audio baseline diff harness vs `pre-vxn-core-extraction`). See
> those tickets for the residual acceptance criteria.

## Summary

Rewire vxn-1 to consume `vxn-core-utils`, `vxn-core-app`,
`vxn-core-ui-web`, and `vxn-core-clap`. Delete the duplicated code
left behind. Prove byte-identical audio against a pre-extraction
baseline. Update vxn-2's open `E003-faceplate` epic to depend on
the shared crates rather than re-implement them.

## Prerequisites

- 0001 (workspace), 0002 (utils), 0003 (app), 0004 (ui-web),
  0005 (clap) all closed.

## Acceptance criteria

- [ ] vxn-1 `vxn-dsp` keeps DSP primitives but re-exports
      `ScopedFlushToZero`, `Smoothed`, `one_pole_coeff`, `note_to_hz`
      from `vxn-core-utils`. Inline copies deleted.
- [ ] vxn-1 `vxn-app` is gutted. The synth-agnostic surface
      (`ParamModel`, `ParamDesc`, `Controller`, `EditorBackend`,
      `UiEvent`/`ViewEvent`, `PresetStore`, sync table) is imported
      from `vxn-core-app`. The synth-specific surface
      (`PatchParam`, `GlobalParam`, `Layer`, `KeyMode`, vxn-1
      cross-mod enums) stays — likely renamed to `vxn-1-params`
      or folded into a smaller `vxn-1-app` shim.
- [ ] vxn-1 `vxn-clap` is gutted. The shell consumes
      `vxn_core_clap::SynthPlugin<VxnEngine>` and provides only the
      `SynthDescriptor`, the engine wiring, and the `gui` extension
      (mounts `vxn-core-ui-web::WebEditor` with vxn-1's HTML).
- [ ] vxn-1 `vxn-ui-web` becomes a thin HTML/CSS/JS asset
      bundler — the `splice_html()` helper that combines vxn-1's
      4 JS modules + CSS + base HTML and returns the `String` for
      `vxn_core_ui_web::open_editor`. WebView lifecycle, IPC,
      text-input popup all gone (moved to shared).
- [ ] vxn-1's existing test suite (`cargo test -p vxn-*` from
      `vxn-1/`) passes unchanged. No tests deleted in this ticket —
      if a test is for code that moved, the test moves with it in
      the prior ticket (0002–0005).
- [ ] **Audio baseline diff.** Before starting this ticket, tag
      vxn-1 main as `pre-vxn-core-extraction`. Render a fixed
      golden patch over a fixed MIDI input (a 60s sequence
      exercising every voice/LFO/FX path) at the tagged commit. After
      migration, render again. Per-sample diff RMS must be < 1e-6
      against the baseline. If non-determinism (RNG, free-running
      LFOs) blocks bit-identity, document the divergent paths and
      use a more permissive tolerance (1e-4 RMS) per path.
- [ ] `vxn-2/epics/open/E003-faceplate.md` updated: the
      `vxn2-app` and `vxn2-ui-web` line items become "implement
      `ParamModel` from `vxn-core-app` for the VXN2 param table"
      and "supply VXN2 HTML to `vxn-core-ui-web::open_editor`".
      Scope drops re-implementation language. Acceptance criteria
      adjusted to reference shared crates.
- [ ] vxn-1 WebView faceplate renders end-to-end. Manual
      verification: load `vxn-1.clap` in Bitwig (or `clack-host`),
      open the editor, turn a knob, observe sound + ViewEvent echo.
- [ ] Root `adrs/0001-vxn-core-split.md` written. Records: what
      was extracted, what was deliberately left synth-local, the
      `Custom` event escape hatch rationale (or assoc-type
      alternative if 0003 went that way), the wire format
      compatibility commitment for state blobs.
- [ ] No new `unwrap`/`expect` in audio-thread paths. No new
      allocations in the process callback (verify via vxn-1's
      existing RT lint or by re-running the relevant benches and
      checking they still allocate zero per block).
- [ ] `cargo bench -p vxn-*` shows no regression > 5% on any
      bench. Audio kernels untouched in this epic; any regression
      points at an unintended dep / inline boundary change.

## Notes

The audio baseline diff is the load-bearing acceptance check. If
extraction perturbs floating-point order anywhere (e.g. by inlining
a smoother differently), bit-identity breaks. The 1e-6 RMS
tolerance is for that case. Hard < 1e-9 is ideal but not realistic
across an LTO boundary change.

vxn-2's E003 update is a docs-only change in this ticket. The
actual implementation of vxn-2's ParamModel impl, HTML faceplate,
and editor wiring is E003's work — this ticket only ensures E003
*can* use the shared crates when it starts.

Order of operations within this ticket:

1. Re-export shims (0002 utils) — smallest blast radius, lands first.
2. Migrate vxn-app surface onto vxn-core-app — touches every UI call site.
3. Migrate vxn-clap onto vxn-core-clap — touches plugin entry only.
4. Migrate vxn-ui-web onto vxn-core-ui-web — touches WebEditor wiring.
5. Run audio baseline diff.
6. Update E003 doc + write ADR.

If any step blows up, revert that step alone — the others are
independent.
