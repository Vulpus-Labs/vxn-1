---
id: "0293"
product: vxn-1b
title: "Browser persistence: IndexedDB user presets, state autosave, patch export/import/share"
priority: medium
created: 2026-08-25
epic: E045
depends: ["0291"]
---

## Summary

Ninth ticket of [E045](../../epics/open/E045-vxn1b-web-wasm-browser-port.md), and
the JS half of the seam [0290](../closed/0290-vxn1b-web-controller-cdylib.md)
built. The controller already keeps a user-preset cache, records a write journal,
hydrates from replayed records, and snapshots / restores full state and TOML —
all tested. None of it reaches storage: `takeJournal()` is drained every pump and
dropped on the floor, so a saved preset lives exactly as long as the tab.

Wire 0284's four shared modules — `preset-storage`, `preset-persistence`,
`state-autosave`, `patch-io` — the way vxn-2 does
([faceplate-bridge.mjs:307](../../vxn-2/crates/vxn2-wasm/web/faceplate-bridge.mjs#L307)),
with VXN1b's own IndexedDB identity so the three corpora never collide.

## Design

### Ordering is the whole ticket

Persistence must complete **before** the queued opcodes are flushed. The page's
boot stub buffers `ready` during parse, and `ready` is what triggers the full
re-broadcast that paints every control — so hydrating and restoring first means
the restored patch is what gets painted and mirrored into the param SAB. Do it
after `install()` and the page paints defaults, then silently disagrees with the
model.

That means moving `bridge.install()` after the persistence step in `boot()`; the
stub keeps queuing in the meantime, which is exactly what it is for.

The corpus publish also moves after hydration, or the browser panel renders the
factory bank alone and never sees the user's folders.

### The journal hook has to change shape

The bridge currently drains the journal itself (`takeJournal()` then hands the
ops to an `onJournal` callback). `PresetPersistence.flush()` *also* drains — it
owns the drain, the write chaining and the availability flag. Both draining means
the pump steals the ops and persistence writes nothing.

So the hook becomes a flush hook the owner drives: the bridge calls it and does
not drain. With no hook wired it must still drain-and-drop, or the wasm journal
grows without bound in a page that has no storage.

### Identity

`{ name: "vxn1b-presets", version: 1 }`, matching `vxn1-presets` / `vxn2-presets`.

### Demo posture

Every path is best-effort, per [[0297]]: persistence here is convenience — your
patch is still there next visit — not durability. Private mode, a blocked
IndexedDB, a quota eviction: log it and carry on with a playable instrument at
defaults. Nothing may throw out of boot.

A share link (`#patch=…`) wins over the autosaved session: it is an explicit
thing the user followed, and restoring the last session over it would silently
discard what they clicked.

### Export / import / share

`patch-io` gives `exportPatchFile` / `importPatchFile` / `shareLinkFor`. There is
no faceplate button for them yet, so expose them on `window.__vxn` as vxn-2 does
— usable from the console and ready for a UI without touching this wiring again.

## Acceptance criteria

- [ ] A user preset saved in one session is present after a reload, with its
      folder, and loads to the same sound.
- [ ] Hydration happens before the queued `ready`, so the restored patch is what
      paints — asserted, not eyeballed.
- [ ] The corpus published to the browser panel carries factory **and** hydrated
      user presets.
- [ ] Rename / move / delete survive a reload (the journal's Delete+Put pairs
      applied in order).
- [ ] The journal is drained exactly once per pump — a test that wiring
      persistence does not leave the pump stealing ops, and that with no
      persistence it still drains so the wasm buffer cannot grow.
- [ ] State autosave restores the last patch on reload; a `#patch=…` share link
      takes precedence over it.
- [ ] Storage failure (no IndexedDB / blocked) leaves a playable instrument and
      logs once — boot does not throw.
- [ ] `window.__vxn.exportPatch` / `importPatch` / `shareLink` work; an exported
      file re-imports to the same patch and is byte-identical to what the plugin
      writes.
- [ ] [0292](0292-vxn1b-xtask-web-pipeline.md)'s `CORE_MODULES` gains the four
      modules; the bundle-closure test enforces it.
- [ ] VXN1b web suite green, 0 skipped; vxn-1 and vxn-2 untouched.

## Notes

- The controller half is done and tested ([[0290]]): journal wire format,
  hydrate opcodes, snapshot/restore, TOML round-trip, and that hydration does
  not journal. This ticket adds no Rust.
- Dist is FLAT and the source tree is not, so the shared modules resolve by
  dynamic import in the browser and by injection in tests — the seam
  [[0294]] introduced for the input adapters. Reuse it.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]. Stage explicit paths —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].

## Close-out (2026-08-27)

Pinned by `persistence.test.mjs`, green inside VXN1b's 151-test suite (0 skipped).

- *"a user preset survives a reload, with its folder and its sound"* — covers the
  reload, the folder and the corpus lookup in one.
- *"boot hydrates BEFORE the queued `ready`, so the restored patch is painted"* —
  the ordering asserted rather than eyeballed, as the criterion asked.
- The published corpus carries factory **and** hydrated user presets
  (`corpusJson().user` walked by `findUserPath`).
- *"rename, move and delete survive a reload"* — the journal's Delete+Put pairs
  applied in order.
- Journal drained exactly once per pump, and still drained with no persistence
  so the wasm buffer cannot grow:
  [persistence.test.mjs:276-281](../../vxn-1b/crates/vxn1b-wasm/web/persistence.test.mjs#L276-L281).
- *"state autosave restores the last patch"* and *"a share link wins over the
  autosaved session"*.
- *"no IndexedDB leaves a playable instrument, and does not throw"*.
- *"an exported patch re-imports to the same sound"* — `exportPatch` /
  `importPatch` / `shareLink`.
- `CORE_MODULES` carries `preset-storage`, `preset-persistence`, `state-autosave`
  and `patch-io` ([main.rs:824](../../vxn-1b/xtask/src/main.rs#L824)); all four
  are in `target/web-dist-vxn1b/` and the closure test enforces the list.
- vxn-1 (29) and vxn-2 (89) suites both green and untouched.
