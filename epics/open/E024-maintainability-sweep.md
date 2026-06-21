---
id: E024
product: vxn-1
title: Maintainability sweep (2026-06-21 review) — finish E001 extraction, untangle render hot path, kill wire-protocol drift
status: open
created: 2026-06-21
---

## Goal

Remediate the net-new findings of the 2026-06-21
maintainability review (four agent sweeps over vxn-dsp,
vxn-engine, vxn-app+vxn-clap, and the web layer). The
review confirmed the architecture is healthy — pure logic
is deliberately extracted for testing, ~400 Rust + JS tests
pass, docs explain *why* — so this is refinement, not
rescue.

One theme dominates: the **E001 core extraction is
half-finished**. The shared `vxn-core-clap` /
`vxn-core-ui-web` crates exist to hold the gnarly
host-echo and WebView plumbing, and vxn-2 already consumes
them correctly — but vxn-1 predates the extraction and
still carries forked copies (already drifting) while
depending on the shared crates. That fork is the
highest-leverage work here and vxn-2 is a working template
for the migration.

The rest: untangle the one logic-rich function the test
suite can only reach end-to-end (`render_block`), put a
CI-failing parity test under the hand-walked binary
ViewEvent protocol, group the two flat god-structs that
keep growing, and dedup a handful of byte-identical codec
paths.

Companion to E011 (the 2026-06-10 review remediation). E011
owns the items it already ticketed — see Relationship
below; this epic does not re-ticket them.

## In scope

- Migrate `vxn-ui-web` onto `vxn_core_ui_web::open_editor`
  + `WebEditorConfig`, deleting the ~250-line fork of
  `EditorHandle`/`WebEditor`/`open_editor`/`ParentWindow`/
  `build_raw`/`ensure_webview2_data_dir`. This also
  discharges E011's "wry build panic → Result" item, since
  the shared `open_editor` already returns `Result`.
- vxn-clap shell dedup: consume `vxn_core_clap::batch_range`
  instead of the inline copy; extract the `push_param_diffs`
  diff/sync-partner logic into a pure, testable function;
  route `value_to_text` through `sync_aware_display` and
  `text_to_value` through `ParamDesc`.
- Extract the per-voice pitch and cutoff assembly out of the
  420-line `render_block` god-function into pure functions,
  giving the osc-routing matrix direct unit tests instead of
  RMS/FFT integration oracles only.
- Group the flat `BlockCtx` (60 fields) into route
  sub-structs; extract a `MasterFx` and an `OutputStage`
  (stereo decimator) so `new`/`set_sample_rate`/`reset` stop
  re-listing every fast-path flag three times.
- Collapse the byte-identical param-table and state codecs
  (`apply_patch_table` ≡ `apply_global_table`; the state
  write/read pairs) onto one generic over a namespace trait.
- Give the core `Controller` a post-load hook so vxn-1 emits
  its key-mode/split events from the load path, removing the
  poll-and-diff shim in the vxn-app `Controller` wrapper.
- Add a golden-byte parity test for the packed ViewEvent
  binary protocol (Rust packs → JS decodes → assert),
  mirroring the existing event-codec golden table.
- Harden the faceplate HTML assembly: explicit placeholder
  tokens instead of `find("<script>")` byte-surgery + the
  `expect` panics; document/strengthen `strip_esm_exports`.
- vxn-dsp structural tidy: gate the dead mono kernels behind
  `#[cfg(test)]`/`pub(crate)` and drop them from the lib
  re-exports; split the 1820-line `poly.rs` into
  `poly/{oscillator,ladder}.rs`.
- Web tree hygiene: delete the stale `vxn-processor-0035/
  0038.js`, `index-0035.html`, and clarify the throwaway
  `vxn-wasm` spike's status.

## Out of scope / Relationship to E011

E011 (2026-06-10) already owns and tickets these — not
re-ticketed here:

- LocalParams gesture-bracket unification → E011 **0017**.
  This epic's clap work (0078) depends on 0017 landing the
  shared `LocalParams<N>`; it adds only `batch_range` and
  the diff/display helpers on top.
- `from_index` transmute, broken `[crate::ladder]` rustdoc
  links, duplicated Padé-tanh coefficient cross-ref (do NOT
  merge — branch/branchless split is deliberate, memory
  `vxn1-tanh-branchless-only`), `HALF_SEMITONE_VOCT` demote
  → E011 **0019**.
- `descriptor_to_json`/`taper_to_json` local-vs-core dedup,
  `#[allow(dead_code)] view_event_to_json`, dead CSS,
  `prototypes/` removal → E011 **0020**.
- FX/mixer param smoothing → E011 **0015**. JS orchestration
  tests → E011 **0016**. CI → E011 (0014/0016).

Also out: any vxn-2 / vxn-3 / monorepo-root work; perf
changes (this epic is behaviour-preserving except where a
ticket says otherwise and re-justifies the baseline hash).

## Tickets

| # | Ticket | Priority |
|---|--------|----------|
| 1 | [0077 — vxn-ui-web: adopt shared vxn-core-ui-web editor](../../tickets/open/0077-ui-web-adopt-core-editor.md) | high |
| 2 | [0078 — vxn-clap: core batch_range + extract diff_params + display dedup](../../tickets/open/0078-clap-shell-dedup.md) | medium |
| 3 | [0079 — vxn-engine: extract pure voice_pitches/voice_cutoff_hz from render_block](../../tickets/open/0079-render-block-extraction.md) | high |
| 4 | [0080 — vxn-engine: group BlockCtx, extract MasterFx + OutputStage](../../tickets/open/0080-engine-struct-grouping.md) | medium |
| 5 | [0081 — vxn-engine: collapse duplicated param-table + state codecs](../../tickets/open/0081-codec-dedup.md) | medium |
| 6 | [0082 — core/wrapper: on_model_loaded hook, drop controller poll-and-diff](../../tickets/open/0082-controller-load-hook.md) | medium |
| 7 | [0083 — web: golden-byte parity test for packed ViewEvent protocol](../../tickets/open/0083-viewevent-parity-test.md) | medium |
| 8 | [0084 — vxn-ui-web: robust HTML assembly via explicit placeholders](../../tickets/open/0084-html-assembly-placeholders.md) | medium |
| 9 | [0085 — vxn-dsp: gate dead mono kernels, split poly.rs](../../tickets/open/0085-dsp-structural-tidy.md) | low |
| 10 | [0086 — web tree hygiene: delete stale spike files](../../tickets/open/0086-web-tree-hygiene.md) | low |

## Acceptance

- vxn-1 has exactly one production WebView editor
  implementation (the shared `vxn-core-ui-web` one) and one
  `batch_range`; the forks are deleted, not commented. A
  forced WebView build failure surfaces as an error and the
  plugin stays alive (E011 wry-panic item discharged here).
- The osc-routing matrix and per-voice cutoff assembly have
  direct unit tests that fail on an arithmetic regression
  without rendering a buffer.
- A layout change to the packed ViewEvent record fails a
  Rust↔JS golden-byte test in CI rather than at runtime.
- `BlockCtx` is grouped into named route sub-structs; the
  master FX chain and output decimator have one `reset`
  each, called from `Synth::reset`/`set_sample_rate` without
  re-listing fast-path flags.
- No byte-identical param-table or state codec remains; the
  shared path is covered by the existing round-trip tests.
- The vxn-app `Controller` wrapper no longer polls the model
  every tick for key-mode/split drift.
- `poly.rs` is split per the one-concept-per-module
  convention; the dead mono kernels are no longer in the
  public re-export list.
- `cargo test --workspace` green; `tests/baseline.rs` render
  hash unchanged (every item is behaviour-preserving).

## Notes

Review source: four agent sweeps, 2026-06-21, over
`vxn-1/crates/{vxn-dsp,vxn-engine,vxn-app,vxn-clap,
vxn-ui-web,vxn-wasm,vxn-web-controller}`. Where line numbers
drift from HEAD, symbol names are authoritative.

Land order: 0077 first (biggest win, vxn-2 is the template,
unblocks the most drift). 0078 waits on E011 0017's shared
`LocalParams`. The rest are independent.
