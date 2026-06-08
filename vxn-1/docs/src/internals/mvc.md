# MVC layering

VXN1 uses an explicit Model / View / Controller split (ADR 0007). The Controller crate is `vxn-app`; it sits between the audio engine, the host, and the GUI, and mediates every non-audio mutation through structured event channels.

The goals:

- The **audio thread never blocks** on the main thread.
- The **GUI is pluggable** — swap Vizia for WebView (we did) without touching the engine.
- The **same Controller serves any instrument** — VXN2 reuses it.

## Roles

### Model

The audio-readable state.

- **`SharedParams`** — flat, index-addressed atomic table of every automatable parameter (156 entries for VXN1). The audio thread reads atomics directly; the main thread mediates writes.
- **Non-automatable state** — Key Mode, Split Point, Layer Switcher selection. Stored in plugin state but not exposed as parameters.

The audio thread has read-only access to non-automatable state via a copy passed at activation time (or via an atomic for fields that can change mid-run, like Key Mode).

### Controller (`vxn-app`)

The mediator. Owns:

- **Event channels** — bounded mpsc rings between (host, GUI) and the Controller; another from the Controller to the GUI.
- **Intent application** — translates `UiEvent` / `HostEvent` into `SharedParams` writes and plugin-state mutations.
- **IO** — preset load/save, file watcher for user preset directory.
- **View updates** — emits `ViewEvent`s when state changes that the GUI needs to know about (preset change, parameter change from host automation, key mode switch).

### View (`vxn-ui-web`)

Stateless presentation layer (except widget tree).

- Receives `ViewEvent`s.
- Emits `UiEvent`s.
- Pluggable — the Controller doesn't depend on `vxn-ui-web` specifically. The current build uses wry-WebView with an HTML/CSS/JS faceplate.

## Event types

```
GUI ──UiEvent──► Controller ◄──HostEvent── CLAP host
                     │
                     ▼
                ViewEvent ──► GUI
                     │
                     ▼
                SharedParams ──► Audio thread (reads atomically)
```

### `UiEvent`

What the user did in the GUI:

- `ParamChanged(id, value)` — knob turned, value entered.
- `PresetLoad(path, target)` — load preset into Upper / Lower / both.
- `PresetSave(meta)` — save current state as preset with metadata.
- `KeyModeChanged(mode)`.
- `SplitPointChanged(note)`.
- `LayerSelected(layer)` — Layer switcher state change.

### `HostEvent`

What the host did:

- `AutomationChanged(id, value)`.
- `MidiCC(cc, value)` — handled by the engine, but the Controller may snoop for routing.
- `ActivateSampleRate(sr)` — triggers engine reactivation.

### `ViewEvent`

What the GUI needs to redraw:

- `ParamUpdated(id, value)` — refresh knob position.
- `PresetLoaded(meta)` — repopulate display, refresh all knobs.
- `PresetListChanged` — file watcher detected user preset directory change.
- `KeyModeChanged(mode)`.
- `Error(message)` — display an error toast.

## Bounded channels

Both directions use bounded mpsc rings:

- **Sender → Controller**: bounded to ~128 events. Overflow drops the oldest non-critical event.
- **Controller → GUI**: bounded to ~256 events; the GUI thread is expected to drain at vsync rate.

The audio thread is not on either of these channels — it reads `SharedParams` atomically and is decoupled from the Controller.

## File IO

Preset load/save runs on the main thread under the Controller, never on the audio thread. The file watcher runs in a background thread that posts `PresetListChanged` events to the Controller.

When a preset loads:

1. Controller parses the TOML.
2. Controller writes each parameter through `SharedParams::set`.
3. Controller posts `ParamUpdated` events to the GUI for each changed knob.
4. Controller signals the CLAP host that parameter values have changed (so DAW automation lanes refresh).

## Why this split

Two driving constraints made the matrix:

1. **Realtime safety** — the audio thread can't allocate, lock, or do file IO. Everything that does happens on the main thread, communicating through atomics and bounded channels.
2. **Reusability** — the next instrument (VXN2) is a DX7-style 6-op FM synth. Different DSP, same Controller / shell / GUI substrate. Splitting the Controller into a generic crate (`vxn-app`) lets VXN2 reuse it verbatim.

ADR 0007 covers the design choices in more depth — particularly why the Controller is a separate crate rather than baked into `vxn-engine`.
