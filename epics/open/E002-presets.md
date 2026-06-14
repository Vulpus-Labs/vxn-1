---
id: E002
product: vxn-1
title: Preset system + Jupiter-8 factory port
status: open
created: 2026-05-28
---

## Goal

Give VXN1 a real preset system: a portable, hand-authorable **TOML** preset
format (keyed by parameter *name*, not index — so it survives the free param-table
reorders), an embedded **factory bank**, a **user** preset directory, a coherent
**load/save** path that keeps `SharedParams` and host automation in sync, and an
in-plugin **browser**. Then the fun payload: port a curated set of
**Jupiter-8-flavoured** factory patches as far as the platform differences allow.

Decisions recorded in [ADR 0005](../../vxn-1/adrs/0005-vxn1-presets.md). This is the
preset-management work that ticket 0007 / ADR 0003 §6 built the state machinery
for and deferred.

## Background

- The state blob (`vxn-engine::state`, `PluginState`) and the per-layer
  serializable unit (`write_patch`/`read_patch`) already exist and stay as the
  **host-session** channel. They are **positional**, so they are *not* the
  portable preset format — CLAP id-stability is dropped and the table reorders
  freely, which would rot any index-keyed preset.
- Every param already carries a stable `ParamDesc.name` and (for enums) variant
  **label** arrays — the keys the TOML format is built on.
- Terminology (ADR 0005 / ticket 0007): **Patch** = one layer's sound (loads into
  Upper or Lower; no global state); **Performance** = both layers + global +
  key mode + split (= `PluginState`).

## Scope

**In:**

- **Preset format + (de)serialization (0024):** `vxn-engine::preset` — serde
  structs, TOML, name-keyed, enum-by-label, sparse-write / default-fill-read,
  `Patch`⟷`PatchValues` and `Performance`⟷`PluginState`, round-trip tests.
- **Factory bank infrastructure + starter set (0025):** `presets/factory/` source
  tree embedded via `include_dir!`; a small set of VXN1-original presets; a CI
  test that parses + round-trips the whole bank.
- **Load/save integration (0026):** bulk parameter load through `SharedParams`
  with host notification (rescan/flush), per-OS user directory IO, Patch→layer
  targeting, Performance applying key mode + split on the non-automatable path.
- **Preset browser UI (0027):** Vizia panel — Factory (by category) + User lists,
  prev/next stepping, current-preset name, Save-As, patch load-target selector.
- **Jupiter-8 factory port (0028):** the fun follow-up — author ~16–24
  archetypal JP-8 sounds as factory presets + a documented mapping/divergence
  table.

**Out (deferred):**

- **CLAP `preset-discovery` / `preset-load` extensions** — clack implements
  neither; raw `clap-sys` FFI, patchy host support. The format + on-disk
  locations are chosen so this layers on later over the same files (ADR 0005 §7).
- A full 64-patch JP-8 ROM clone (we have no original data; 0028 is honest
  archetypes).
- Preset tagging/search UI beyond category grouping; favourites; preset morphing.
- Bundle-resource (`Contents/Resources`) preset staging — embedding is primary.

## Tickets

- [x] 0024 — Preset format + (de)serialization
- [x] 0025 — Factory bank infrastructure + starter set
- [x] 0026 — Preset load/save integration + host notify
- [ ] 0027 — Preset browser UI
- [ ] 0028 — Jupiter-8 factory preset port

## Dependency order

```text
0024 (format) ──┬──> 0025 (factory infra + starter set) ──┐
                ├──> 0026 (load/save + host notify) ───────┴──> 0027 (browser UI)
                └──> 0028 (JP-8 port — authored against the format,
                            auditioned once 0027 lands)
```

0024 is foundational (owns the schema + conversion). 0025 and 0026 are
independent of each other and both build on 0024; 0027 needs both (a browser that
lists *and* loads). 0028 only needs the format (0024) to author files, but wants
0027 to audition them — land it last, or iterate its files after 0027.

## Acceptance

- A preset is a name-keyed TOML file; reordering the param table does not break
  existing presets; unknown keys warn-and-skip; absent keys take descriptor
  defaults; enums are stored by label. Every shipped factory preset parses and
  round-trips in CI.
- A **Patch** loads into a chosen layer without disturbing global state or the
  other layer; a **Performance** restores both layers + global + key mode +
  split. A bulk load updates `SharedParams` and the host's displayed/automation
  values, with no audio-thread allocation.
- The browser lists Factory (by category) + User presets, steps prev/next, shows
  the current name, and saves to the per-OS user directory.
- The JP-8 set loads and plays; each preset documents (inline comment) the
  notable divergence from its hardware inspiration; the mapping table is recorded
  in 0028.
