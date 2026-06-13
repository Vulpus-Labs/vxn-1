---
id: E010
title: Standalone executables (macOS + Windows) for vxn-1 & vxn-2
status: open
created: 2026-06-13
depends-on: E009
---

> **Depends on E009.** The Windows standalone hosts a Windows `.clap`,
> which E009 produces and verifies. The macOS standalone can start as
> soon as this epic opens. Note: this epic spans **both** synths — vxn-1
> is already Windows-capable, vxn-2 becomes so via E009.
>
> **Coordination with vxn-1 E020 (VST3 via clap-wrapper, ADR 0008).**
> E020 already adopts clap-wrapper in **bundled / single-binary
> (static-link) mode** to ship VST3, and already plans the
> `vendor/clap-wrapper` submodule + `wrapper/CMakeLists.txt` (E020
> 0109/0110) and the `staticlib` crate-type on `vxn-clap` (E020 0108).
> This epic **reuses all three** rather than duplicating them, and
> reconciles to the same static link mode (see "Why static, not
> embedded" below). E020's "Standalone format — no demand" out-of-scope
> line is superseded by this epic. If E020's scaffold lands first, 0103
> here becomes a thin "extend the shared scaffold to standalone +
> vxn2-clap staticlib" ticket.

## Goal

Ship standalone desktop apps for vxn-1 and vxn-2 — a double-clickable
`.app` (macOS) and `.exe` (Windows) that runs the synth with audio
output, MIDI input, and the HTML editor, **without a host or DAW**.

Use [`free-audio/clap-wrapper`](https://github.com/free-audio/clap-wrapper)'s
standalone target. clap-wrapper *is* a CLAP host: it provides audio I/O
(RtAudio), MIDI in (RtMidi), an OS window + menu, and drives the
plugin's `gui` extension exactly as a DAW does. It links the CLAP in
**bundled / single-binary (static) mode** — the same mode vxn-1's E020
uses for VST3 — so the standalone is self-contained with no runtime
`.clap` file to locate. This needs each plugin's clap crate to expose a
`staticlib` archive (see "Why static, not embedded").

When this epic closes:

- `vxn1-standalone.app` / `vxn2-standalone.app` run on macOS;
  `VXN1.exe` / `VXN2.exe` run on Windows.
- Each opens its WebView editor, makes sound from a MIDI keyboard, and
  lets the user pick audio/MIDI devices.
- CI produces standalone artifacts alongside the plugin bundles.

## Why this approach

The plugin core is host-agnostic and the `gui` extension uses the
**embedded** window model (`is_floating` is rejected; the host creates a
window and calls `set_parent`). clap-wrapper's standalone creates a
top-level window and calls `set_parent` — identical to the DAW path the
editor already runs under. So the wry editor attaches as a child with no
new code.

The alternative — a hand-rolled Rust shell (cpal + winit + midir) —
means writing and maintaining audio-device, MIDI, and window plumbing
per OS. clap-wrapper already ships all three, battle-tested across
shipping products. The cost is a CMake + C++ build step layered on top
of the existing `cargo xtask` flow.

### Why static, not embedded

clap-wrapper can either **embed** a `.clap` file inside the artifact and
load it at runtime, or **statically link** the CLAP archive into the
standalone binary (bundled mode). This epic uses **static**, matching
vxn-1 E020's VST3 decision (ADR 0008), because:

- **One Rust-side pattern for both formats.** E020 already adds
  `staticlib` to `vxn-clap` (0108) and smoke-tests that clack's entry
  macro exports `clap_entry` from the archive. Standalone reuses that;
  vxn-2 gets the same `staticlib` addition to `vxn2-clap`. No separate
  embedded-path plumbing.
- **Self-contained, no path resolution.** No bundled `.clap` to locate
  relative to the `.app`/`.exe` at launch — the code is in the binary.
- **Shared scaffold.** The same `vendor/clap-wrapper` submodule and
  `wrapper/CMakeLists.txt` serve VST3 and standalone.

Trade-off: this epic is **no longer "zero Rust change"** — `vxn2-clap`
must gain `crate-type = ["cdylib", "rlib", "staticlib"]` (vxn-clap is
covered by E020 0108). That change is small and isolated to the clap
crate's manifest + an entry-symbol smoke link.

## Scope

**In:**

- Add `staticlib` to `vxn2-clap`'s crate-type and smoke-link the
  `clap_entry` export from the archive (vxn-clap is covered by E020
  0108). Reuse / share E020's `vendor/clap-wrapper` submodule +
  `wrapper/CMakeLists.txt` rather than vendoring a second copy.
- A minimal CMake project that invokes `target_add_standalone_wrapper`
  against each synth's CLAP **static archive** (bundled / single-binary
  mode).
- An `xtask standalone` subcommand per synth that builds the staticlib
  slice(s), then runs CMake to assemble the standalone app.
- macOS standalone for vxn-1 and vxn-2 (`.app` bundles): RtAudio out,
  RtMidi in, window hosting the wry editor.
- Windows standalone for vxn-1 and vxn-2 (`.exe`): RtAudio / RtMidi /
  WebView2.
- CI jobs that produce the standalone artifacts (mac + win) for both
  synths.
- Short user docs: launching, audio/MIDI device selection, the WebView2
  runtime prerequisite on Windows.

**Out:**

- Linux standalone (clap-wrapper supports x11/gtk, but no Linux plugin
  CI exists yet — defer with the Linux plugin work).
- VST3 wrapping — already its own epic (vxn-1 E020 / ADR 0008); shares
  this epic's wrapper scaffold + static link mode. AU (AUv2/AUv3) is a
  later follow-up off the same wrapper.
- Standalone-specific features (built-in sequencer, audio recording,
  preset I/O beyond what the plugin already does).
- Any change to the plugins' DSP, params, or editor.

## Tickets

- [ ] [0103 — Vendor clap-wrapper + minimal CMake scaffold](../../tickets/open/0103-clap-wrapper-scaffold.md)
- [ ] [0104 — vxn-1 macOS standalone (.app) + xtask standalone](../../tickets/open/0104-vxn1-macos-standalone.md)
- [ ] [0105 — vxn-2 macOS standalone (.app)](../../tickets/open/0105-vxn2-macos-standalone.md)
- [ ] [0106 — vxn-1 Windows standalone (.exe)](../../tickets/open/0106-vxn1-windows-standalone.md)
- [ ] [0107 — vxn-2 Windows standalone (.exe)](../../tickets/open/0107-vxn2-windows-standalone.md)
- [ ] [0108 — CI: standalone artifacts (mac + win) for both synths](../../tickets/open/0108-standalone-ci.md)
- [ ] [0109 — Docs: standalone usage + device selection + WebView2 prereq](../../tickets/open/0109-standalone-docs.md)

## Dependency order

```text
0103 (clap-wrapper scaffold)
  │
  ├─> 0104 (vxn-1 macOS) ─> 0105 (vxn-2 macOS)
  │         │                     │
  │         └──────┬──────────────┘
  │                ▼
  │         0106 (vxn-1 Windows)*  ─> 0107 (vxn-2 Windows)*
  │                                         │
  └──────────────────────────────────┬─────┘
                                      ▼
                            0108 (CI) ─> 0109 (docs)

* 0106/0107 require E009 closed (a verified Windows VXN2.clap; vxn-1 is
  already Windows-capable).
```

- 0104 proves the whole approach on the lowest-risk OS (all paths
  already exercised on macOS). 0105 is then mostly config (different
  `.clap`, bundle id, window size).
- Windows tickets gate on E009 — do not start them until vxn-2's
  Windows plugin is green and its editor verified.
- 0108 generalises the build once at least one mac + one win standalone
  works; 0109 documents the shipped behaviour.

## Risks

- **Toolchain creep.** Adds CMake (≥ 3.21) and a C++ compiler (clang on
  macOS, MSVC on Windows) to the build. RtAudio / RtMidi come via
  clap-wrapper's `FetchContent`. CI images already have these, but local
  dev now needs them for the standalone path (the plugin path stays
  pure-cargo).
- **macOS main-thread / AppKit.** wry on macOS requires the main thread.
  clap-wrapper drives the `NSApp` run loop and calls `gui` on the main
  thread, matching what the editor already assumes — low risk, but the
  first launch is the proof.
- **Windows first light.** Inherits E009's WebView2 + win32 HWND
  surface. The standalone's window is clap-wrapper's HWND rather than a
  DAW's; the editor and `WS_POPUP` text-input popup must anchor to it
  correctly. Verify after E009, not before.
- **staticlib entry-symbol export.** Static mode requires clack's entry
  macro to emit `clap_entry` from the `.a`/`.lib` archive, not just the
  cdylib. E020 0108 smoke-tests this for `vxn-clap`; confirm the same
  for `vxn2-clap` before wiring CMake. (Static link removes the
  embedded-`.clap` path-resolution risk entirely — nothing to locate at
  runtime.)
- **Universal macOS.** vxn-1 ships a `lipo` universal `.clap`; confirm
  clap-wrapper's standalone packaging carries both slices (or build the
  standalone per-arch and `lipo` after).

## Acceptance

- Double-clicking `vxn1-standalone.app` / `vxn2-standalone.app` on macOS
  and `VXN1.exe` / `VXN2.exe` on Windows launches the synth, opens the
  WebView editor, and produces audio from a connected MIDI keyboard.
- Audio output device and MIDI input device are selectable (via
  clap-wrapper's standard standalone menu).
- The hosted `.clap` is bundled inside the standalone artifact — no
  dependency on an installed plugin.
- CI uploads standalone artifacts for both synths on mac and win.
- No change to plugin DSP, params, editor, or the `.clap` bundles
  themselves — the standalone is purely additive.
- Docs cover launch, device selection, and the WebView2 runtime prereq.
