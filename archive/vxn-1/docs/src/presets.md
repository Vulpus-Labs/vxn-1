# Presets

VXN1 has two kinds of preset:

- **Patch** — one layer's state. Loads into either the Upper *or* Lower layer; doesn't touch the other layer, global, or key mode.
- **Performance** — the full instrument state: both layers, global parameters, key mode, and split point. Equivalent to capturing the whole plugin.

Both kinds share a TOML file format. Factory presets ship inside the plugin binary; user presets live on disk and are fully editable.

## File format

Plain TOML. Keys are **parameter names** (matching the labels in this manual), not numeric IDs — so files survive parameter reordering. Values are in display units (Hz, seconds, semitones, etc.). Enums are stored by label, case-insensitive.

The format is **sparse**: only parameters that differ from the descriptor default are written. **Forward-compatible**: unknown keys are silently skipped, missing keys fall back to defaults.

```toml
schema = 1
kind = "patch"

[meta]
name = "Brass Ensemble"
author = "Vulpus Labs"
category = "Brass"

[patch]
osc1_wave = "Saw"
osc2_octave = -1
cutoff = 4200.0
resonance = 0.35
env1_attack = 0.05
env2_sustain = 0.9
chorus_mix = 0.6
```

The `[meta]` table currently carries `name`, `author`, and `category`. Free-text tags were specced in early drafts but dropped from the format — there is no `tags` field on disk.

For Performances, the structure splits across multiple tables:

```toml
schema = 1
kind = "performance"

[meta]
name = "Split Bass + Lead"
author = "Vulpus Labs"

[performance.upper]
# Upper layer params

[performance.lower]
# Lower layer params

[performance.global]
master_volume = 0.65
chorus = true
delay_mix = 0.3
key_mode = "Split"
split_point = 60
```

## Storage

| OS | Location |
| --- | --- |
| **macOS** | `~/Library/Audio/Presets/Vulpus Labs/VXN1/` |
| **Windows** | `%APPDATA%\Vulpus Labs\VXN1\Presets` |
| **Linux** | `$XDG_DATA_HOME/VXN1/presets` (fallback: `~/.local/share/VXN1/presets`) |

Inside the preset root, folder structure is **flat one level deep** — categories are subdirectories, presets are files in those directories. No nested categories.

Factory presets are **embedded in the binary** at compile time via `include_dir!` and are read-only at runtime.

## Factory bank

Seven categories ship in the factory bank:

- Bass
- Brass
- Keys
- Lead
- Pad
- Performance
- Strings

The Performance category specifically holds full-instrument states (Dual layered patches, Split keyboards) — patches with `kind = "performance"`.

## Browser

The preset browser opens from the **Browse** button on the preset bar at the top of the faceplate. It shows:

- **Folder tree** on the left (categories + your subfolders).
- **Preset list** on the right.
- **Search box** above the list — substring match on preset name.
- **Context menu** on each preset: Rename, Delete, Move to ▸.
- **Drag-drop** — drag a user-preset row onto a user folder in the left pane to move it into that folder. Factory folders are not drop targets; the source folder is rejected too. The context menu's "Move to ▸" is the fallback.

Factory presets have read-only flags — you can't rename or delete them, but you can save a modified copy via **Save As**.

## Load semantics

- **Loading a Patch** writes to one layer (the Layer switcher chooses Upper or Lower). Global, key mode, split point, and the other layer are untouched.
- **Loading a Performance** replaces everything. The plugin announces the change to the host so DAW automation lanes refresh.

There's no "load undo" — load is destructive of the previously edited state. Save your in-progress patch before loading a new one if you want to come back to it.

## Save form

The **Save As** dialog asks for:

- **Name** — required. Used as the filename (with extension `.toml`).
- **Author** — optional, persists to `[meta] author`.
- **Category** — picks the folder. Pre-filled with the current category or empty for a new one.

Saves to user storage only; factory presets are read-only.
