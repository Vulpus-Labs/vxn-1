---
id: "0364"
product: monorepo
title: "WebView editor steals the host's keyboard: make the faceplate keyboard-transparent to the DAW"
priority: high
created: 2026-09-04
epic: null
depends: []
---

## Summary

A user reports that with a Vulpus plugin's editor open, the DAW stops receiving
keyboard input — spacebar transport, computer-MIDI keyboard, undo. Not
reproducible in REAPER; the reports across the plugin-dev world point at Ableton
Live and Logic on macOS, and FL Studio on Windows.

This is the generic embedded-WebView focus problem, not a per-synth bug. All
three synths mount the same editor —
[vxn-core-ui-web/src/lib.rs:466](../../crates/vxn-core-ui-web/src/lib.rs#L466),
`WebViewBuilder::new_as_child` over the host's parent handle from
[vxn-core-clap/src/gui.rs:30](../../crates/vxn-core-clap/src/gui.rs#L30) — so
there is one place to fix and it is the shared crate.

Mechanism, per platform (read from the wry 0.45 sources):

- **macOS.** Child mode is a bare `[host_ns_view addSubview: wkwebview]`. A click
  makes the WKWebView's internal content view the window's `firstResponder`;
  WebKit then consumes `keyDown:` and the host's responder chain never sees the
  event. (wry's own `makeFirstResponder` call is on the standalone-window path,
  which we don't use — we lose focus to the click, not to wry.)
- **Windows.** wry creates a container HWND of class `WRY_WEBVIEW` as a direct
  child of the host HWND; WebView2 nests its own HWNDs under that. Once clicked,
  the thread's focus lives in that subtree and keystrokes go to the browser
  process.

Neither is reachable from the plugin API: CLAP has no key-forwarding extension,
and the VST3 wrapper's `IPlugView::onKeyDown` never fires because the native
control consumes the event upstream of it. The fix has to be native.

The good news is that the WebView barely needs the keyboard. Preset rename /
save-as / new-folder already bypass it entirely — `ViewEvent::OpenTextInput` is
intercepted in
[push_view_event](../../crates/vxn-core-ui-web/src/lib.rs#L339) and dispatched to
the floating native popup in
[text_input.rs](../../crates/vxn-core-ui-web/src/text_input.rs#L1-L13), whose
header already names this exact host behaviour ("hosts swallow Space and friends
for transport before any child NSView sees them"). What is left in-page:

- the preset-browser search field —
  [faceplate.html:52](../../vxn-1b/crates/vxn1b-ui-web/assets/faceplate.html#L52),
  [index.html:537](../../vxn-2/crates/vxn2-ui-web/assets/index.html#L537). The
  only real text input the plugins have. vxn-3's faceplate is still a stub with
  no inputs.
- Escape-to-close: preset panel
  ([preset-browser.js:145](../../crates/vxn-core-ui-web/assets/preset-browser.js#L145)),
  curve picker
  ([curve-picker.js:107](../../crates/vxn-core-ui-web/assets/curve-picker.js#L107)),
  matrix overlay
  ([matrix.js:284](../../vxn-1b/crates/vxn1b-ui-web/assets/panels/matrix.js#L284)).
- the roving `btn.focus()` in the matrix combo picker.

## Design

Policy: **the WebView gets the mouse; the host gets the keyboard, unless the page
has explicitly claimed it.**

That the mouse half already works without focus is established —
`with_accept_first_mouse(true)` at
[lib.rs:470](../../crates/vxn-core-ui-web/src/lib.rs#L470) exists precisely so a
click reaches an unfocused WebView.

### 1. Focus yank, on the tick we already have

`EditorHandle::flush_view_events`
([lib.rs:366](../../crates/vxn-core-ui-web/src/lib.rs#L366)) runs on every
`on_timer` in all three clap shells (e.g.
[vxn1b-clap/src/lib.rs:552](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L552)) at
~60 Hz. Hooking the guard there means **zero per-synth changes** — no new call
site, no new trait method.

- **macOS.** Read `firstResponder` off `[webview window]`; if it `isDescendantOf:`
  the WKWebView, `makeFirstResponder: parent_ns_view`, falling back to `nil` (the
  window itself) if that returns `NO`. The WKWebView pointer comes from
  `wry::WebViewExtMacOS::webview()`; the parent NSView is already held on the
  handle for the text popup.
- **Windows.** Cache the container HWND at open —
  `FindWindowExW(parent_hwnd, null, w!("WRY_WEBVIEW"), null)`. Each tick, if
  `GetFocus()` is that window or `IsChild` of it, `SetFocus(parent_hwnd)`.
- **Linux.** No-op, same precedent as the `text_input.rs` stub.

Cost when idle is two pointer reads per tick. Worst case is one 16 ms window in
which the page holds the keyboard — shorter than any human click-to-keypress gap.

Guard against interrupting a gesture: skip the yank while a mouse button is down
(`[NSEvent pressedMouseButtons] != 0` / `GetCapture()`). macOS mouse tracking is
anchored on the view that took `mouseDown:`, not on the first responder, so this
should be redundant — but a knob drag dying mid-gesture is the expensive failure
mode and the check is two instructions.

**No synthetic key injection.** Forwarding focus back to the parent is the whole
mechanism. The `SendInput` approach other developers have used for FL Studio is
what they then report as unreliable, and it is untestable on the hosts available
here.

### 2. Keyboard claim from the page

New shared opcode `want_keyboard { on }` in
[parse_ui_event_default](../../crates/vxn-core-ui-web/src/lib.rs#L557). The IPC
closure and the `EditorHandle` share an `Arc<AtomicBool>`; no controller
round-trip, since nothing in the model cares. While the flag is set the yank is
skipped, and the rising edge calls `webview.focus()` once so the claim takes
effect on the same tick.

Page side, in the shared assets so every faceplate inherits it:

- `focusin` / `focusout` on `input, textarea, [contenteditable]` → claim /
  release. Covers the search field.
- Explicit claim while a modal, curve picker or matrix overlay is open. Covers
  Escape and the arrow-key nav.
- Release on `window` blur, as a watchdog against a stuck claim.

### 3. Runtime opt-out

`VXN_WEBVIEW_KEYBOARD=1` disables the yank entirely and restores today's
behaviour. Read once in `open_editor`. This is a support lever, not a
convenience: Logic, FL Studio and Cubase can't be tested here (see Notes), and a
user hitting a regression in one of them needs a fix that doesn't require a
rebuild.

## Acceptance criteria

- [ ] Reproduced first, in Ableton Live on macOS: with the editor open and
      clicked, spacebar does not transport and the computer-MIDI keyboard does not
      play. Recorded in the close-out. If it does not reproduce, this design is
      addressing the wrong thing and the ticket stops here.
- [ ] Live/macOS after the fix: spacebar transports, computer-MIDI keyboard plays,
      Cmd+Z undoes — all after clicking the faceplate.
- [ ] Live/Windows (Parallels) after the fix: same three, with Ctrl+Z.
- [ ] REAPER/macOS and REAPER/Windows: the same three still work (control — REAPER
      was never broken), and the page-side behaviour is unregressed: the preset
      search field still accepts typing, Escape still closes the preset panel, the
      curve picker and the matrix overlay, and a knob drag is not cut off
      mid-gesture.
- [ ] `VXN_WEBVIEW_KEYBOARD=1` restores the pre-fix behaviour, verified in one host.
- [x] The guard lives in `vxn-core-ui-web` alone — `git diff --stat` shows no
      change under `vxn-1b/crates/vxn1b-clap`, `vxn-2/crates/vxn2-clap` or
      `vxn-3/crates/vxn3-clap`.
- [x] Unit coverage for the parts that are testable without a host: the
      `want_keyboard` opcode round-trips through `parse_keyboard_claim` (and
      pointedly *not* through `parse_ui_event_default` — see Notes), and the JS
      claim/release logic has a vitest case per trigger (text focus, overlay
      open, window blur).
- [x] `cargo test --workspace` green; the vxn-1b and vxn-2 JS suites green with
      their pass counts unchanged. 1678 / 0 workspace, 382 vxn-1b (369 + 13 new),
      35 vxn-2.
- [x] The Windows branch type-checks, even though it can't be run here:
      `cargo check -p vxn-core-ui-web --target x86_64-pc-windows-msvc` clean.

## Found while doing this

- **The claim is not a `UiEvent`.** The plan said the opcode would round-trip
  through `parse_ui_event_default`; it doesn't, and shouldn't. Native focus is a
  property of the window plumbing, not of the patch, so adding a variant would
  have obliged all three controllers to carry a match arm they only ever ignore.
  It is intercepted in the IPC handler by `parse_keyboard_claim` instead, ahead
  of `parse_ui_event`, and a test pins that `parse_ui_event_default` still
  refuses the opcode so the two can't quietly both grow one.
- **Five claim call sites, not four.** vxn-2's mod-matrix overlay is opened from
  `main.js` but *closed* from two places (`main.js`'s `close_mod_matrix` and
  `mod-matrix.js`'s own backdrop / close-button path), and it has a second
  overlay the survey missed — the algo picker in `panels/op-row.js`, with its own
  Escape binding. Releasing an already-released token is a no-op, which is what
  makes the double-close path harmless.
- **A prose comment broke the page-assembly assertion.** `vxn1b-ui-web`'s
  `esm_exports_stripped` asserts the spliced faceplate contains no bare
  `im`+`port ` token; a comment in the new asset that read "those import the
  factory above" tripped it. Reworded, with a note in the asset explaining why
  the phrasing is load-bearing. Worth knowing before writing the next shared
  asset — the check is a substring match, not a parse.
- **Cross-checking Windows needs the pinned toolchain, not `stable`.** `rustup
  target add x86_64-pc-windows-msvc` installs under 1.95.0 (the
  `rust-toolchain.toml` pin), so the check has to run through
  `~/.rustup/toolchains/1.95.0-*/bin/cargo`. Forcing `RUSTC` at `stable` — the
  reflex from [[wasm-build-toolchain]] — points it at a 1.96.1 that has no
  Windows std and fails with `can't find crate for core`, which reads like a
  broken target install rather than a wrong toolchain.

## Notes

- **Host coverage is two hosts, and that is defensible.** Only REAPER and Live are
  available, on macOS and on Windows under Parallels. The fix has exactly two
  implementations — the macOS `firstResponder` yank and the Windows `SetFocus` —
  and Live exercises both. Logic runs the same WKWebView path as Live/macOS; FL
  Studio runs the same WebView2 path as Live/Windows. Cubase is the genuine gap:
  it installs its own key hooks and is the plausible place for "hand focus back to
  the parent" to land somewhere unexpected. Accepted risk, covered by the env-var
  opt-out.
- **Parallels caveat when testing.** The VM needs the WebView2 Evergreen runtime
  (the `WEBVIEW2_USER_DATA_FOLDER` override at
  [lib.rs:273](../../crates/vxn-core-ui-web/src/lib.rs#L273) is already in place).
  Use a real USB keyboard, or check Parallels' Cmd→Ctrl remapping first — a
  remapped shortcut failing to reach the host looks identical to this bug.
- Best coverage for the untestable hosts: ship behind the env var and ask the
  reporting user to try a beta. They have the host that broke.
- wry 0.45 has `WebView::focus()` but no `focus_parent()` — that landed in a later
  release. Both native branches are hand-rolled here (`objc` on macOS,
  `windows-sys` on Windows), and `vxn-core-ui-web` already depends on both for the
  text popup, so no new dependencies.
- Out of scope: moving the preset search field to a native popup (it would kill
  search-as-you-type); any change to `text_input.rs`, which already sidesteps this
  problem; a Linux implementation; keyboard accessibility / tab navigation of the
  faceplate, which this policy deliberately does not attempt.
- No `cargo fmt` — [[vxn-no-cargo-fmt]]; hand-format the new Rust.
- Stage explicit paths, never `git add -A` —
  [[vxn-concurrent-vxn2-work-no-git-add-all]].
