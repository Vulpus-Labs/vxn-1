---
id: "0053"
product: vxn-1
title: "Web MIDI: access request, input enumeration, hotplug"
priority: high
created: 2026-06-15
epic: E017
depends: ["0042"]
---

## Summary

Request Web MIDI access from a `WebHost`, enumerate the present input ports,
subscribe each one's `onmidimessage`, and handle device hotplug
(`statechange` connect/disconnect) without a page reload. Graceful on denial /
unavailability so the computer-keyboard fallback (0055) can still come up.

## Design

- **Module** `vxn-1/crates/vxn-wasm/web/midi-input.mjs` — `attachMidi(host, opts)`.
- **Access.** `navigator.requestMIDIAccess({ sysex })` behind an injectable seam
  (`opts.requestMIDIAccess`) so the Node harness can supply a fake (Node has no
  Web MIDI). `sysex` defaults false (we never need SysEx).
- **Enumeration.** Iterate `access.inputs.values()`, subscribe each `type ===
  "input"` port (outputs skipped) by setting `port.onmidimessage`. Idempotent
  (a re-subscribe of an already-wired id is a no-op).
- **Hotplug.** `access.onstatechange` filters to input ports: `connected`
  subscribes, `disconnected` unsubscribes. Reconnect with the same id
  re-subscribes cleanly. `opts.onStateChange(port)` surfaces the change to the
  page (E018 can list devices).
- **Graceful denial.** If `requestMIDIAccess` is absent (Safari w/o Web MIDI) or
  the prompt rejects, RESOLVE (never throw) a controller with
  `state.granted === false` and call `opts.onError(err)`, so the page falls back
  to keyboard input — the epic's safety-net contract.
- **Return** `{ access, state, inputs(), detach() }`. `detach()` clears
  `onstatechange` and every port's `onmidimessage`.

## Acceptance criteria

- [ ] `attachMidi` requests access via the (injectable) `requestMIDIAccess` and
      reports `state.granted`.
- [ ] All present input ports are subscribed on attach; output ports skipped.
- [ ] A `statechange` connect adds + subscribes a new input; a disconnect
      removes + unsubscribes it — no reload.
- [ ] Denial / unavailability resolves a `granted === false` controller and
      calls `onError`; never throws.

## Notes

- The decode itself (bytes → producer calls) + the timestamp→offset map are
  0054; this ticket is the access/enumeration/hotplug plumbing. They ship in the
  same module file.
- Single-timbral: all 16 MIDI channels fold onto the one engine (matches
  vxn-clap, which ignores the channel nibble).

## Close-out (2026-06-15)

- **Module.** `vxn-1/crates/vxn-wasm/web/midi-input.mjs` — `attachMidi(host,
  opts)`. Requests access via the injectable `requestMIDIAccess` seam (default
  `navigator.requestMIDIAccess`, bound), subscribes every `type === "input"`
  port's `onmidimessage`, skips outputs, idempotent re-subscribe.
- **Hotplug.** `access.onstatechange` filters to input ports: `connected`
  subscribes, `disconnected` unsubscribes; `onStateChange(port)` surfaces it.
- **Graceful denial.** Absent `requestMIDIAccess` or a rejected prompt RESOLVES
  a `state.granted === false` controller and calls `onError` — never throws, so
  the keyboard fallback (0055) comes up. Returns `{ access, state, inputs(),
  detach() }`.
- **Tests.** `web/midi-input.test.mjs` §3 (enumeration: 2 inputs subscribed,
  output skipped, live message reaches ring, detach unsubscribes), §4 (hotplug
  add/remove + onStateChange + hotplugged port plays), §5 (graceful denial:
  no-API and rejected-prompt both `granted=false`, `onError` fired). Fakes a
  MIDIAccess + ports; asserts via the real `EventRing.drainInto`.
- **Build.** Added `midi-input.mjs` to the xtask `web()` MODULES copy-list;
  `cargo run -p vxn1-xtask -- web` bundles it into `target/web-dist/`.
- **Headless run note.** The `node web/midi-input.test.mjs` harness is written
  and self-reviewed but could not be executed inside this build sandbox (node
  script execution is not permitted here). Run it manually to capture the
  PASS lines; CI/manual is the gate.
