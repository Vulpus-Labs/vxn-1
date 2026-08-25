# vxn-core-web

The browser-glue JS shared by every VXN web port. One copy, in
[`assets/`](assets) — extracted by ticket 0284 when VXN1b became the third port
and six of the fourteen glue modules turned out to be duplicates.

| module | what it is |
|---|---|
| `preset-storage.mjs` | the raw IndexedDB primitive (three object stores, no corpus logic) |
| `preset-persistence.mjs` | async-storage ↔ sync-controller bridge: boot hydration + write-behind journal flush |
| `state-autosave.mjs` | full patch-state autosave/restore — the host-state-blob analogue |
| `patch-io.mjs` | patch export/import as `.toml` + `#patch=` share-link |
| `midi-input.mjs` | Web MIDI → event-ring producer |
| `keyboard-input.mjs` | computer keyboard → event-ring producer |

## What is *not* shared

The eight remaining modules in each port's `web/` directory — `event-ring`,
`param-store`, `event-codec`, `coordinator`, `controller`, `audio-host`,
`host-runner`, `faceplate-bridge` — encode per-synth model shape and diverge by
50–1300 lines. They stay forked deliberately.

Nor is **configuration**. Two values differ per synth and are passed in by the
caller rather than baked into the shared source:

- the IndexedDB identity, `{ name, version }` — `openPresetDB`'s required
  argument, forwarded by `PresetPersistence` / `StateAutosave` via their `dbId`
  option. The name partitions one synth's corpus from another's in the same
  origin; the version is that database's own migration history. Getting either
  wrong evicts or blocks a user's live presets, so there is no default.
- the product name — `exportPatchFile` / `importPatchFile`'s `product` option,
  which names the default download and the rejection message.

`cargo test -p vxn-core-web` fails if a shared module grows a hardcoded value
for either.

## How a port consumes these

`dist/` is flat: `xtask web` copies these modules in alongside the port's own,
so in the browser they resolve as plain `./x.mjs` siblings. The source tree is
not flat, so nothing in a port's `web/` imports them statically — the bridges
take them as **injected options**, matching the seam idiom the modules already
use for `openDB`, timers and `WebHostClass`. Each port's tests import them by
their real relative path; only the browser uses the flat-sibling default.
