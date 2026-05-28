# ADR 0005 — VXN1 preset packaging & format

- **Status:** Accepted
- **Date:** 2026-05-28
- **Scope:** How VXN1 stores, ships and loads presets: the on-disk file format,
  the patch-vs-performance distinction, where factory and user presets live, and
  how a load interacts with the parameter / automation model. This is the
  preset-management ADR that ticket 0007 and ADR 0003 §6 deferred (referred to
  there as "ADR 0004"; that number went to oscillator interaction, so the preset
  ADR is 0005).

## Context

The state machinery has been built preset-ready from the start and the moment to
use it has arrived: we want to **ship sounds**, including a Jupiter-8-flavoured
factory set (ticket 0028).

What already exists (do **not** rebuild):

- `vxn-engine::state` — a canonical binary blob (`PluginState::write/read`,
  magic `VXN1` + `VERSION`) wired into the CLAP `state` extension. This is the
  **host-session channel**: the DAW serializes it into the project file.
- A per-layer block (`PatchValues`, the `PatchParam` table) is already a
  **self-contained serializable unit** (`write_patch`/`read_patch`), explicitly
  so a single-layer sound can load into one layer (ticket 0007 §Notes).
- `ParamDesc.name` — every param carries a stable string name (`"osc1_wave"`,
  `"cutoff"`, …) and enum params carry their variant **label** arrays
  (`WAVE_LABELS`, `FILTER_MODE_LABELS`, …).

Two facts force the format decision:

1. **CLAP id-stability is dropped** ([[vxn1-id-stability-dropped]], ADR 0004 /
   E004). The `PatchParam` / `GlobalParam` tables are reordered freely for
   clarity. **Any preset keyed by parameter index or CLAP id therefore rots** the
   next time the table is touched — which is exactly why the binary `state` blob,
   which *is* positional, is unsuitable as the portable preset artifact (it is
   gated by an exact `VERSION` and hard-rejects on any layout change).
2. **Presets are a long-lived, shippable, hand-authorable artifact.** Factory
   presets must survive engine refactors; the JP-8 port (0028) is authored by
   hand from synthesis recipes and wants comments and readability.

ADR 0002's "Explicitly not doing → the plugin host owns preset state" stands only
for *session* state (the host still owns that, via the `state` blob). It is
**superseded here for portable presets**: VXN1 ships its own preset files and an
in-plugin browser, because host-native preset systems vary wildly and cannot
carry a curated factory bank across DAWs.

## Terminology

Reusing ticket 0007's distinction, with fixed names:

- **Patch** — one layer's sound: a `PatchValues` (the `PatchParam` block). Loads
  into **either** layer (Upper or Lower). Carries **no** global state (no FX, no
  master, no LFO 2 rate/shape — those are not part of a layer; cf. ticket 0019).
- **Performance** — the full instrument: both layers' patches + the `GlobalParam`
  block + `KeyMode` + split point. Equivalent to `PluginState`. This is what a
  Dual/Split setup needs.

(The JP-8 lineage: a JP-8 "patch" is a single timbre ≈ our **Patch**; its
Dual/Split panel-memory combination ≈ our **Performance**.)

## Decision

### 1. Format: TOML, keyed by parameter **name**, values in plain units

Presets are **TOML** text files. Rationale over the binary blob and over JSON:

- **Name-keyed, not index-keyed** — the single most important choice. Keys are
  `ParamDesc.name` strings. A loader resolves name → param, so reordering the
  param table (which we do freely) never corrupts a preset. Unknown keys are
  **skipped with a warning** (forward-compatible: a preset written by a newer
  build still loads its known params); absent keys fall back to the **descriptor
  default** (presets are **sparse** — only deviations from default are stored, so
  files are small, diffable, and auto-adopt improved defaults).
- **Enums stored by label, not index** — `osc1_wave = "Saw"`, `filter_mode =
  "LP"`, `cross_mod_type = "FM"`. Reordering or inserting enum variants does not
  silently re-map a preset. Loader matches against the descriptor's variant
  labels (case-insensitive); unmatched → default + warning.
- **Bools** as `true`/`false`; **floats/ints** as numbers in the descriptor's
  plain unit (Hz, st, s, ct, fraction). Clamped on load (`PatchValues::set`
  already clamps to range).
- TOML over JSON: comments (the JP-8 port documents each patch inline),
  human-editable, line-diffable, mature `toml` crate.

The binary `state` blob is **unchanged** and **kept** for the CLAP host-session
channel — it is fast, opaque, and never hand-edited. The two formats serve
different jobs; we do **not** unify them.

### 2. File schema

```toml
schema = 1                  # preset *file-format* version (independent of state VERSION)
kind   = "patch"            # "patch" | "performance"

[meta]
name     = "Brass Ensemble"
author   = "Vulpus Labs"
category = "Brass"          # browser grouping
tags     = ["jp8", "poly"]  # optional, free-form
comment  = "..."            # optional

# kind = "patch": one [patch] table of PatchParam name = value (sparse)
[patch]
osc1_wave = "Saw"
cutoff    = 4200.0
resonance = 0.35
# ... only non-default params appear
```

```toml
schema = 1
kind   = "performance"

[meta]
name = "Split Bass / Lead"

[performance]
key_mode    = "Split"       # "Whole" | "Dual" | "Split"
split_point = 48            # MIDI note (kind=performance only)

[performance.global]        # GlobalParam names (sparse)
master_volume = 0.7
chorus_on     = true

[performance.upper]         # PatchParam names (sparse)
osc1_wave = "Saw"

[performance.lower]
osc1_wave = "Pulse"
```

A bumped `schema` signals an incompatible file change. Because the format is
name-keyed, most evolutions need **no** bump (add/rename via an alias table);
reserve `schema` for structural changes.

### 3. Conversion lives in `vxn-engine`

A new `vxn-engine::preset` module owns the pure mapping, framework-free and
RT-irrelevant (called from the UI/main thread, never audio):

- `Patch` ⟷ `PatchValues`, `Performance` ⟷ `PluginState`.
- serde `Serialize`/`Deserialize` structs + `to_toml_string` / `from_toml_str`.
- **Sparse on write** (omit any param equal to its descriptor default),
  **default-fill on read**; name/label lookup helpers over `PATCH_PARAMS` /
  `GLOBAL_PARAMS`; lossless round-trip tests for every param.

### 4. Factory presets are **embedded at compile time**

Factory presets ship **inside the binary** via `include_dir!` over a source tree
`crates/vxn-engine/presets/factory/<category>/*.toml`. No install step, nothing
to lose, identical across DAWs and OSes, and the round-trip test suite can parse
the whole bank at build/test time (a malformed factory preset fails CI).

(Rejected as the primary mechanism: copying presets into the `.clap` bundle's
`Contents/Resources` — bundle-relative path discovery from a loaded cdylib is
fiddly and OS-specific. We may *additionally* stage them on disk later for the
preset-discovery route in §7, but the in-plugin browser reads the embedded set.)

### 5. User presets live in a per-OS writable directory

Save-As writes `.toml` to a user directory the browser also scans:

- **macOS:** `~/Library/Audio/Presets/Vulpus Labs/VXN1/`
- **Linux:** `$XDG_DATA_HOME/VXN1/presets` (fallback `~/.local/share/VXN1/presets`)
- **Windows:** `%APPDATA%\Vulpus Labs\VXN1\Presets`

The browser shows **Factory** (embedded, read-only, grouped by `category`) and
**User** (on disk, writable) sources.

### 6. Load semantics & the parameter/automation model

A preset load is a **bulk parameter write** and must stay coherent with the
single-source-of-truth `SharedParams` and with host automation:

- **Patch load** targets **one layer** (Upper / Lower / the currently-edited
  layer in the UI). It writes only that layer's `PatchValues`; global state, the
  other layer, key mode and split point are **untouched**. (This is what lets you
  stack two factory patches into a Dual/Split performance.)
- **Performance load** replaces everything: both layers, the global block, and
  the non-automatable `KeyMode` + split point.
- All automatable writes go **through `SharedParams`** and are **announced to the
  host** so its automation lanes / displayed values update — the bulk analogue of
  the UI's existing per-edit gesture echo. The mechanism (emit per-param
  values on the next flush, or a CLAP params **rescan(VALUES)** /
  `request_flush`) is pinned in ticket 0026; it must not allocate on the audio
  thread.
- `KeyMode` + split point are **not** CLAP params; they are applied directly on
  the same non-automatable shared-state path the `state` blob already uses, and a
  Performance that changes key mode honours the existing seed-on-entry rules
  (ADR 0003 §3).

### 7. CLAP preset-discovery / preset-load: **deferred, designed-for**

The CLAP `preset-discovery` factory and `preset-load` extension would let the
host index our files and show them in its own browser. We **defer** them:

- clack does **not** implement either (the `preset_discovery` module is empty
  stubs; there is no `preset-load`), so it is raw `clap-sys` FFI.
- Host support is patchy (Bitwig yes; many hosts no).

But the §2 file format and the §4/§5 on-disk locations are deliberately chosen so
a later preset-discovery **provider can index the very same `.toml` files by
absolute path** with no format change — the deferred route layers cleanly on top
of the in-plugin browser, sharing one format and one preset corpus.

## Consequences

- New `vxn-engine` dependencies: `serde` (derive) + `toml`, and `include_dir`
  for the factory embed. All main-thread only; the audio path is untouched.
- A new `presets/factory/` source tree becomes a first-class, CI-checked
  artifact: every shipped preset must parse and round-trip.
- The browser is new UI surface (ticket 0027) and the bulk-load host-notify path
  is the one genuinely delicate integration (ticket 0026).
- Sparse + name-keyed presets **auto-adopt** improved descriptor defaults and
  survive table reorders — but a default *change* silently shifts old presets
  that relied on the old default; acceptable pre-release, and authors can pin a
  value by writing it explicitly.
- The JP-8 port (0028) is **best-effort archetypes**, not a ROM clone: we have no
  original patch data and the platforms diverge (see that ticket's mapping
  table). Framed honestly as "Jupiter-flavoured", not "the JP-8 factory bank".

## References

- ADR 0001 — overall design (the "three writers: host automation, UI edits,
  preset load" note; `SharedParams` as single source of truth).
- ADR 0002 — feature roadmap (§"Explicitly not doing": host owns *session* state;
  superseded here for *portable* presets).
- ADR 0003 §3/§6/§8 — two-layer param model, key mode + split as non-automatable
  shared state, seed-on-entry.
- ADR 0004 / E004 — fixed-panel param model; CLAP id-stability dropped.
- Ticket 0007 — param-block split; the Patch / Patch-Preset-pair distinction and
  the self-contained `write_patch`/`read_patch` unit.
- Ticket 0019 — a single-layer patch does not carry global LFO 2 / FX.
- CLAP spec — `clap.preset-discovery` factory and `clap.preset-load` extension
  (deferred, §7).
