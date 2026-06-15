---
id: "0061"
product: vxn-1
title: "DOM text-input popup (replace the desktop floating input)"
priority: medium
created: 2026-06-15
epic: E018
depends: ["0058"]
---

## Summary

Re-home the rename/save text-input popup to the DOM. The plugin opens a native
floating NSWindow/HWND outside the host's key-event scope (`request_text_input`
opcode -> `ViewEvent::OpenTextInput` -> native popup). On the web there is no
host key-event scope and no native window, so the popup is a plain DOM overlay:
the bridge intercepts `request_text_input` and resolves it locally without
crossing into the controller.

## Design

- **Intercept in the bridge.** `request_text_input` is NOT forwarded to the
  controller (the controller's `RequestTextInput` only matters for native, where
  it routes to the NSWindow). The bridge opens a DOM modal (a centered input over
  a backdrop), and on commit/cancel calls the page's `text_input_result`
  dispatch path directly: `window.vxn.onViewEvent({kind:'text_input_result', id,
  value})`. `value` is the string on Enter, `null` on Esc / click-outside —
  matching the plugin's contract so `bridge.js`'s `_textInputCallbacks` fires
  exactly once.
- **Reuse the page's callback plumbing.** `window.vxn.promptText` already posts
  `request_text_input` and stashes a callback keyed by id; the bridge only has to
  deliver the `text_input_result` back. No faceplate change needed beyond what
  already exists.

## Acceptance criteria

- [ ] A `request_text_input` opcode opens a DOM input overlay (not forwarded to
      the controller).
- [ ] Commit delivers `{kind:'text_input_result', id, value:<string>}`; cancel
      delivers `value:null`; the page's promptText callback fires once.

## Notes

- Depends on [0058](0058-web-bridge-js-to-controller.md).
- The overlay rendering needs a real browser to verify visually; the
  resolve-once contract is headless-testable.

## Close-out (2026-06-15)

- **Intercept + popup.** [faceplate-bridge.mjs](../../vxn-1/crates/vxn-wasm/web/faceplate-bridge.mjs):
  `request_text_input` is intercepted by `handleUiOpcode` (NOT forwarded to the
  controller) and opens `openTextInputPopup` — a DOM modal (backdrop + centered
  input). On Enter it delivers `{kind:'text_input_result', id, value:<string>}`;
  on Esc / click-outside it delivers `value:null`, straight to
  `window.vxn.onViewEvent` so bridge.js's one-shot `_textInputCallbacks` fires
  exactly once. The popup CSS (`.vxn-ti-*`) is injected into the web page's boot
  head ([vxn-ui-web build_web_faceplate_html](../../vxn-1/crates/vxn-ui-web/src/lib.rs)),
  no faceplate-asset edit needed.
- **Verified headlessly.** [faceplate-bridge.test.mjs](../../vxn-1/crates/vxn-wasm/web/faceplate-bridge.test.mjs)
  drives a minimal fake DOM: the popup seeds the initial value, Enter delivers
  `text_input_result` once (id + value), a second Enter does NOT re-deliver
  (fire-once), and the backdrop is removed on commit. Run with `node` manually
  (see 0058 caveat) — not exercised in the authoring sandbox.
- Browser-only: the overlay's visual rendering + focus behaviour need a real
  browser via `cargo xtask web --serve`.
