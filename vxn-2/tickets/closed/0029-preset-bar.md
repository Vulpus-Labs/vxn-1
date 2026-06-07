---
id: "0029"
title: Preset bar wiring (Init round-trip; Save / SaveAs / Browse stubs)
priority: medium
created: 2026-06-06
closed: 2026-06-07
epic: E003
---

## Summary

Wire the preset bar: show the current preset name (`"Init"` until
the preset epic ships), prev / next steppers (no-op until a corpus
exists — log "no presets" status), and Save / SaveAs / Browse
buttons that fire `UiEvent::SavePreset` / `UiEvent::EditorReady`-
ish stubs the controller logs and discards.

The default patch from 0018 IS the first patch the user sees:
loading the plugin shows "Init" in the preset bar, every control
reflects the patch, tweaking is audible. This ticket establishes
the round-trip even though the actual preset format / factory
bank ship in a later epic.

## Acceptance criteria

- [ ] Preset name display: reads from the latest
      `ViewEvent::PresetLoaded { meta }`. Falls back to `"Init"`
      on first open (no preset has been loaded — the controller
      emits a synthetic `PresetLoaded { name: "Init" }` on
      `UiEvent::EditorReady`, after `broadcast_all_params`).
- [ ] Prev / next steppers: dispatch
      `step_preset { delta: -1|+1 }` opcodes. With an empty
      corpus, the controller emits
      `ViewEvent::Status { line: "No presets available" }`; the
      preset bar shows this as a transient toast.
- [ ] Save button: dispatch
      `save_preset { name: <current display name> }`. Controller
      logs to `Status { line: "Save not yet supported in this
      build" }`. No filesystem write.
- [ ] Save As button: opens the native text-input popup (0030)
      seeded with the current preset name; on commit dispatches
      `save_preset { name: <new>, folder: null }`. Controller
      logs "Save As not yet supported".
- [ ] Browse button: opens a placeholder modal that lists "No
      presets yet"; closes on Escape / outside-click. The button
      is reachable from keyboard navigation.
- [ ] Controller addition: when `vxn2-app::tick_vxn2` receives a
      `UiEvent::EditorReady` from the page, it emits a
      `ViewEvent::PresetLoaded { meta: PresetMeta { name:
      "Init", category: None, ..Default::default() }, source:
      None, warnings: vec![] }` before the
      `broadcast_all_params` so the bar paints "Init" on the
      first tick.
- [ ] Manual smoke: open the editor, see "Init" in the preset
      bar; press Save → toast "Save not yet supported"; press
      Save As → text input pops, commit closes the popup +
      toast; press Browse → modal shows the empty state.

## Notes

- This ticket deliberately stubs the disk I/O. The preset epic
  on top of E003 (vxn-1 E007 lineage) wires the real
  `PresetStore` impl, the factory bank, and the browser modal.
  We're proving the buttons fire, the events flow, and the bar
  refreshes — not the full preset format.
- The synthetic "Init" `PresetLoaded` on `EditorReady` should
  fire ONCE per editor session. If the controller already has a
  loaded preset (in a future tick after the preset epic lands),
  skip the synthetic emit. For E003 this never happens.
- Browse modal: just a `<dialog>` element with role / aria
  attributes — keyboard close path matters for accessibility
  even at the empty-state stage.
- Don't add a real factory bank here. Embedding factory
  patches into the binary is preset-epic work (ADR 0005 §3 /
  §4). The Init patch lives in `vxn2_engine::default_patch`
  and is sourced via the parameter defaults, not a preset blob.
