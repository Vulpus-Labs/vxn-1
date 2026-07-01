# Standalone apps

VXN1 and VXN2 ship as standalone applications in addition to CLAP (and VST3) plugins.
The standalone embeds the full synth engine plus the HTML faceplate editor into a
self-contained binary — no DAW required for sound design or live use.

## Building

From the workspace root (inside `vxn-1/` or `vxn-2/` respectively):

```sh
# VXN1 — macOS universal (arm64 + x86_64)
cargo xtask standalone --release --universal

# VXN1 — single arch (current machine)
cargo xtask standalone --release

# VXN2 — macOS / Windows
cd vxn-2
cargo xtask standalone --release
```

Prerequisites (in addition to Rust): **CMake 3.21+** and a C++ compiler.
The `clap-wrapper` and `clap` submodules must be initialised first:

```sh
git submodule update --init --recursive
```

Output lands in `target/bundled/`:

| Synth | macOS | Windows |
| --- | --- | --- |
| VXN1 | `target/bundled/VXN1.app` | `target/bundled/VXN1.exe` |
| VXN2 | `target/bundled/VXN2.app` | `target/bundled/VXN2.exe` |

## Launching

**macOS** — double-click `VXN1.app` or `VXN2.app` from Finder, or from the
terminal:

```sh
open target/bundled/VXN1.app
# or
target/bundled/VXN1.app/Contents/MacOS/VXN1
```

**Windows** — double-click `VXN1.exe` or `VXN2.exe`.

The app window opens with the synth faceplate. Audio and MIDI are
started automatically using the default system devices; use the **Devices**
menu (top of the window) to switch to a different output or input.

## Device selection

The standalone provides a menu bar (macOS) or system tray / title-bar menu
(Windows) with the following entries:

- **Audio** → lists available audio outputs. Select any entry to switch;
  the engine restarts on the new device with the current patch intact.
- **MIDI** → lists available MIDI inputs. Multiple inputs can be enabled
  simultaneously. Plug in a controller and it should appear without restarting.

The default sample rate and buffer size are set by the device; the standalone
does not currently expose per-device latency tuning (follow-up).

## Windows — WebView2 runtime

The HTML faceplate editor requires the **Microsoft Edge WebView2 runtime**.

On **Windows 10 (2004 and later)** and **Windows 11** the runtime ships as
part of the OS and no installation is needed.

On older Windows builds (or a clean VM), install it from:
<https://developer.microsoft.com/microsoft-edge/webview2/>

The standalone will still launch without WebView2, but the editor panel
will be blank — only the Devices menu is usable. Presets and audio function
normally even without the editor.

## macOS — first-launch prompts

**Gatekeeper** — unsigned builds (from source) trigger a Gatekeeper warning
on first launch:

> "VXN1.app" cannot be opened because it is from an unidentified developer.

Clear it with:

```sh
xattr -d com.apple.quarantine target/bundled/VXN1.app
```

or, for a relocated copy:

```sh
xattr -rd com.apple.quarantine /Applications/VXN1.app
```

See [Unsigned binaries](install-unsigned.md) for more detail.

**Microphone / audio permissions** — the standalone requests audio output
permission on first launch. Allow it in **System Settings → Privacy &
Security → Microphone** if the prompt is dismissed or the system denied it.
