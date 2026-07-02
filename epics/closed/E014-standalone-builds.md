---
id: E014
product: vxn-2
title: Standalone executables (macOS + Windows) for vxn-1 & vxn-2
status: shelved
created: 2026-06-13
shelved: 2026-07-02
depends-on: E013
---

> **SHELVED 2026-07-02.** Standalone builds are dropped for now. The
> macOS standalone via clap-wrapper hit two blockers that make it a poor
> deliverable without patching the vendored wrapper:
> - **Dead faceplate.** clap-wrapper's macOS standalone host does not
>   implement `timer-support` (`register_timer` returns `false`; no
>   run-loop drives `on_timer`), so the editor's diff/flush pump never
>   runs and the UI never echoes host/controller activity.
> - **Mic-permission crash.** RtAudio probes input devices at startup,
>   tripping macOS TCC; the bundle must carry `NSMicrophoneUsageDescription`
>   (a spurious mic prompt) or the app is hard-killed — even though the
>   synth opens no audio input.
> All standalone code, the `standalone/` CMake project, the RtAudio/RtMidi
> pull, the `cargo xtask standalone` commands, and the Build-Standalone CI
> job were removed. The shared clap-wrapper submodule and the `staticlib`
> crate-type stay — VST3 (E010) still uses them. Revive by patching the
> wrapper's macOS standalone (NSTimer-driven `on_timer`; skip input
> enumeration for 0-input plugins) or by hand-rolling a Rust shell.

> **Depends on E013.** The Windows standalone hosts a Windows `.clap`,
> which E013 produces and verifies. The macOS standalone can start as
> soon as this epic opens. Note: this epic spans **both** synths — vxn-1
> is already Windows-capable, vxn-2 becomes so via E013.
>
> **Coordination with vxn-1 E010 (VST3 via clap-wrapper, ADR 0008).**
> E010 already adopts clap-wrapper in **bundled / single-binary
> (static-link) mode** to ship VST3, and already plans the
> `vendor/clap-wrapper` submodule + `wrapper/CMakeLists.txt` (E010
> 0009/0010) and the `staticlib` crate-type on `vxn-clap` (E010 0008).
> This epic **reuses all three** rather than duplicating them, and
> reconciles to the same static link mode (see "Why static, not
> embedded" below). E010's "Standalone format — no demand" out-of-scope
> line is superseded by this epic. If E010's scaffold lands first, 0027
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
**bundled / single-binary (static) mode** — the same mode vxn-1's E010
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
vxn-1 E010's VST3 decision (ADR 0008), because:

- **One Rust-side pattern for both formats.** E010 already adds
  `staticlib` to `vxn-clap` (0008) and smoke-tests that clack's entry
  macro exports `clap_entry` from the archive. Standalone reuses that;
  vxn-2 gets the same `staticlib` addition to `vxn2-clap`. No separate
  embedded-path plumbing.
- **Self-contained, no path resolution.** No bundled `.clap` to locate
  relative to the `.app`/`.exe` at launch — the code is in the binary.
- **Shared scaffold.** The same `vendor/clap-wrapper` submodule and
  `wrapper/CMakeLists.txt` serve VST3 and standalone.

Trade-off: this epic is **no longer "zero Rust change"** — `vxn2-clap`
must gain `crate-type = ["cdylib", "rlib", "staticlib"]` (vxn-clap is
covered by E010 0008). That change is small and isolated to the clap
crate's manifest + an entry-symbol smoke link.

## Scope

**In:**

- Add `staticlib` to `vxn2-clap`'s crate-type and smoke-link the
  `clap_entry` export from the archive (vxn-clap is covered by E010
  0008). Reuse / share E010's `vendor/clap-wrapper` submodule +
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
- VST3 wrapping — already its own epic (vxn-1 E010 / ADR 0008); shares
  this epic's wrapper scaffold + static link mode. AU (AUv2/AUv3) is a
  later follow-up off the same wrapper.
- Standalone-specific features (built-in sequencer, audio recording,
  preset I/O beyond what the plugin already does).
- Any change to the plugins' DSP, params, or editor.

## Tickets

- [ ] [0027 — Vendor clap-wrapper + minimal CMake scaffold](../../tickets/open/0027-clap-wrapper-scaffold.md)
- [ ] [0028 — vxn-1 macOS standalone (.app) + xtask standalone](../../tickets/open/0028-vxn1-macos-standalone.md)
- [ ] [0029 — vxn-2 macOS standalone (.app)](../../tickets/open/0029-vxn2-macos-standalone.md)
- [ ] [0030 — vxn-1 Windows standalone (.exe)](../../tickets/open/0030-vxn1-windows-standalone.md)
- [ ] [0031 — vxn-2 Windows standalone (.exe)](../../tickets/open/0031-vxn2-windows-standalone.md)
- [ ] [0032 — CI: standalone artifacts (mac + win) for both synths](../../tickets/open/0032-standalone-ci.md)
- [ ] [0033 — Docs: standalone usage + device selection + WebView2 prereq](../../tickets/open/0033-standalone-docs.md)

## Dependency order

```text
0027 (clap-wrapper scaffold)
  │
  ├─> 0028 (vxn-1 macOS) ─> 0029 (vxn-2 macOS)
  │         │                     │
  │         └──────┬──────────────┘
  │                ▼
  │         0030 (vxn-1 Windows)*  ─> 0031 (vxn-2 Windows)*
  │                                         │
  └──────────────────────────────────┬─────┘
                                      ▼
                            0032 (CI) ─> 0033 (docs)

* 0030/0031 require E013 closed (a verified Windows VXN2.clap; vxn-1 is
  already Windows-capable).
```

- 0028 proves the whole approach on the lowest-risk OS (all paths
  already exercised on macOS). 0029 is then mostly config (different
  `.clap`, bundle id, window size).
- Windows tickets gate on E013 — do not start them until vxn-2's
  Windows plugin is green and its editor verified.
- 0032 generalises the build once at least one mac + one win standalone
  works; 0033 documents the shipped behaviour.

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
- **Windows first light.** Inherits E013's WebView2 + win32 HWND
  surface. The standalone's window is clap-wrapper's HWND rather than a
  DAW's; the editor and `WS_POPUP` text-input popup must anchor to it
  correctly. Verify after E013, not before.
- **staticlib entry-symbol export.** Static mode requires clack's entry
  macro to emit `clap_entry` from the `.a`/`.lib` archive, not just the
  cdylib. E010 0008 smoke-tests this for `vxn-clap`; confirm the same
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
