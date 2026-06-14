---
id: E019
product: vxn-2
title: "vxn-1 web port — persistence (presets + state)"
status: open
created: 2026-06-14
depends-on: E018
---

> **Depends on E018** (the UI emits the preset opcodes this epic backs)
> and on E015's controller placement. Replaces the desktop `std::fs`
> preset I/O with browser storage behind the existing `PresetStore` trait,
> and provides full-state save/load (the host-state-blob analogue).

## Goal

Make presets and patch state persist in the browser. The factory bank
ships embedded (already wasm-compatible via `include_dir`); user presets
move to IndexedDB or OPFS behind the existing `PresetStore` abstraction;
full patch state can be saved and restored (export/import + autosave).

When this epic closes:

- Factory presets load in the browser (embedded bank verified under wasm).
- User presets save/load/rename/delete/move via browser storage, driven by
  the same opcodes the faceplate already emits.
- Full patch state survives reload (autosave to storage) and can be
  exported/imported as a file or URL for sharing.

## Why a dedicated epic

Persistence is the one place the engine touches `std::fs`
(`preset_io.rs`), which is a no-op under wasm. The `PresetStore` trait
already abstracts storage, so this is a clean swap — but browser storage
is async (IndexedDB/OPFS), which the synchronous trait and the
once-per-tick controller loop must accommodate. That impedance is worth
isolating.

## Background

- The factory bank is embedded with `include_dir`
  (`vxn-engine`) — compile-time assets, wasm-compatible, should "just
  work."
- User preset I/O uses `std::fs` behind a `PresetStore` trait
  (`EnginePresetStore`); on wasm, `std::fs` calls return `Unsupported`.
- Full plugin state is a binary blob the host persists; on the web there
  is no host, so the page owns save/load (storage + file + URL).

## Scope

**In:**

- Verify the embedded factory bank loads under wasm.
- A browser-storage `PresetStore` impl (IndexedDB or OPFS — decide in the
  first ticket) for user presets: list, load, save, rename, delete, move,
  folder ops — matching the opcode surface.
- Bridge the async storage API to the synchronous controller loop
  (e.g. main-thread cache hydrated on boot, writes fire-and-forget).
- Full-state autosave/restore on reload.
- Patch export/import: download/upload a patch file, and/or an
  encode-in-URL share link.

**Out:**

- Cloud sync / accounts.
- Preset format changes — reuse the existing name-keyed TOML
  (`vxn1-preset-system`).
- The preset *browser UI* itself (E018) — this epic backs its opcodes.

## Planned tickets

> Ids assigned at scaffold time. Provisional set:

- [ ] Verify embedded factory bank under wasm.
- [ ] Choose + implement browser-storage `PresetStore` (IndexedDB/OPFS).
- [ ] Async-storage ↔ sync-controller bridge (boot hydration + deferred
      writes).
- [ ] Full-state autosave/restore on reload.
- [ ] Patch export/import (file + URL share link).

## Risks

- **Async vs sync impedance.** IndexedDB/OPFS are async; the controller
  expects synchronous `PresetStore`. The hydration/deferred-write pattern
  must not stall the audio path or drop writes.
- **Storage quotas + eviction.** Browser storage can be evicted; surface
  this and prefer durable storage where available.
- **OPFS vs IndexedDB maturity.** OPFS is newer (better fit for file-ish
  data) but less universal; the first ticket decides.

## Acceptance

- Factory presets load in the browser.
- User presets round-trip through browser storage via the existing
  opcodes (save/load/rename/delete/move/folders).
- Patch state survives a page reload.
- A patch can be exported and re-imported (file and/or URL).
- No preset-format change vs the desktop build.
