---
id: "0003"
title: vxn-core-app — ParamModel, Controller, events, backend
priority: high
created: 2026-06-06
epic: E001
---

## Summary

Extract vxn-1's `vxn-app` crate (~1900 LOC) into shared
`vxn-core-app`, made synth-agnostic. Owns the UI-facing parameter
schema, the UI↔engine event types, the `Controller` event loop, the
`EditorBackend` trait, and the `PresetStore` trait. Per-synth param
enums (vxn-1's `PatchParam`, vxn-2's 380-param registry) stay in
their own crates and implement `ParamModel` against this surface.

## Acceptance criteria

- [ ] `vxn_core_app::params::ParamModel` trait — `get(id) -> f32`,
      `set(id, value)`, `default(id)`, `descriptor(id) -> &ParamDesc`,
      `iter() -> impl Iterator<Item = (ParamId, &ParamDesc)>`.
      `ParamId` is a newtype around `u32` (CLAP id space). No
      synth-specific enum baked in.
- [ ] `ParamDesc` carries name, label, `ParamKind`
      (Float / Int / Bool / Enum), min, max, default, taper, optional
      enum-variant list. Match vxn-1 fields exactly.
- [ ] `Taper` enum: `Linear`, `Exponential { midpoint: f32 }`. Move
      the existing taper math intact.
- [ ] `vxn_core_app::events::UiEvent` — UI → Controller. Variants:
      `BeginGesture(ParamId)`, `EndGesture(ParamId)`,
      `ParamSet { id: ParamId, value: f32 }`, `PresetLoad(PresetId)`,
      `PresetSaveAs { name: String, category: String }`,
      `TextInput { id: ParamId, text: String }`, … plus a
      `Custom(Box<dyn Any + Send>)` escape hatch for per-synth events
      that don't belong in the shared schema (key-mode change, layer
      select, etc.). Per-synth events ride `Custom`; shared events
      stay typed.
- [ ] `vxn_core_app::events::ViewEvent` — Controller → UI. Variants:
      `ParamChanged { id, value }`, `PresetLoaded(PresetMeta)`,
      `PresetCorpusChanged`, `Status(String)`, plus `Custom(...)`
      symmetric escape hatch.
- [ ] `vxn_core_app::backend::EditorBackend` trait — `open`,
      `close`, `push_event(ViewEvent)`, `flush_events()`. No
      mention of wry / WebView; the trait is GUI-framework-agnostic.
- [ ] `vxn_core_app::controller::Controller<M: ParamModel, B: EditorBackend>`
      — tick-driven event loop. `handle_ui_event`, `tick(host_state)`,
      `push_view_event`. No threads, no timers; the host (CLAP shell
      or test harness) drives `tick`.
- [ ] `vxn_core_app::preset::PresetStore` trait — `list()`,
      `load(id)`, `save(meta, snapshot)`, `delete(id)`,
      `rename(id, new_name)`, `move_to(id, new_category)`. Generic
      over a `Snapshot` assoc type (per-synth patch shape).
- [ ] `vxn_core_app::preset::PresetCorpus` — factory bank +
      user-folder snapshot type. Category-grouped, recursive folders
      under user. Mirror vxn-1's shape.
- [ ] vxn-1's `Layer`, `KeyMode`, `DEFAULT_SPLIT_POINT`,
      `UNCATEGORIZED` and similar must NOT be in this crate. They
      stay in vxn-1's app crate (or a new `vxn-1-app` after 0006).
- [ ] Crate compiles standalone; unit tests cover taper math
      (Linear roundtrips identically; Exp midpoint == midpoint),
      `Controller` event-loop ordering (UiEvent → engine call →
      ViewEvent echo), and `PresetCorpus` (de)serialization round-trip
      against a fixed JSON fixture.
- [ ] No deps on `wry`, `clack`, or any specific synth crate. Pure
      logic + `serde` for preset JSON.

## Notes

The hardest call in this ticket is how to handle synth-specific event
payloads (vxn-1's `KeyModeChanged(Whole | Dual | Split { point })`,
vxn-2's mod-matrix-row edits). Two options:

**A. `Custom(Box<dyn Any + Send>)` escape hatch.** Shared events
typed; per-synth events ride dynamic. Loose but simple. UI bridge
serialises `Custom` payloads via a per-synth `serde` impl supplied
when the controller is constructed.

**B. `Controller<M: ParamModel, Ext: EventExt>` with assoc types.**
`EventExt::Ui` and `EventExt::View` extend the variants. Type-safe
but every match site needs to know `Ext`.

Recommend A for v1 — matches vxn-1's existing shape and avoids
ripping every match site. Revisit if vxn-2 finds the dyn cost or
the loss of exhaustiveness painful.

Drop preset format (vxn-1's `LayerBlock` / `GlobalBlock` schema) —
that is synth-specific, stays in vxn-1. Only the `PresetStore` trait
+ `PresetCorpus` model are shared.

`Controller` event-loop semantics (gesture brackets, dedup,
ordering) must be preserved exactly — vxn-1's WebView depends on
them. Lift them as-is and document in a `// Why:` comment if they
look surprising.
