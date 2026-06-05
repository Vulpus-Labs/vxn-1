---
id: "0013"
title: "Editor: key mode, layer toggle, split point"
priority: medium
created: 2026-05-25
epic: E003
---

## Summary

Editor support for key modes (ADR 0003 §6): a `KeyMode` selector, an Upper/Lower
**edit-target toggle** that switches which per-patch block the faceplate edits
(hidden in Whole), and a split-point control. The faceplate stays single — the
toggle picks which layer's params the controls bind to.

## Acceptance criteria

- [x] Key-mode selector (Whole / Dual / Split) writing the non-automatable
      `KeyMode` shared state (0007), via the state path — **not** a param
      gesture (same mechanism as the split point below).
- [x] Upper/Lower toggle selects the edit target; faceplate controls bind to the
      selected layer's per-patch ids, so a gesture writes that layer's **fixed**
      CLAP id (the host records the specific `Upper_*`/`Lower_*` param — ADR 0003
      §6). The toggle is hidden/disabled in Whole (editing layer A only).
- [x] A clear visual indication of which layer is being edited (and, in
      Dual/Split, that the other layer is active but not shown).
- [x] Split-point control (note value) visible in Split mode, writing the opaque
      split-point state (0009) via the appropriate host/state path (it is not an
      automatable param, so it is set through state, not a param gesture).
- [x] Switching the toggle re-points all faceplate bindings to the other layer's
      values without spurious automation writes (no echo, no gesture on mere
      view switch).

## Notes

- Mirrors the JP-8 workflow: one panel, a Lower/Upper focus, the display showing
  which patch you are editing.
- Reuses the existing Vizia faceplate; the change is parameter **binding
  indirection** (layer-relative id resolution) plus two new controls.
- Split-point-as-state (not a param) means the UI writes it through a
  non-automation path; confirm the cleanest mechanism against the state impl
  from 0007/0009.
- Depends on 0008 (layers) and 0009 (mode/split state). Validation: build the
  editor and exercise mode/toggle/split manually; add binding unit tests where
  feasible.
