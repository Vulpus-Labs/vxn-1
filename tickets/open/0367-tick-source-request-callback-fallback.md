---
id: "0367"
product: monorepo
title: "TickSource in vxn-core-clap — drive the editor from request_callback when the host has no timer-support"
priority: high
created: 2026-09-10
epic: E051
depends: []
---

## Summary

The faceplate is alive only for as long as a host grants a timer. Each plugin
registers one in `set_parent` and drains ViewEvents from `on_timer`
([vxn2-clap/src/gui.rs:48-60](../../vxn-2/crates/vxn2-clap/src/gui.rs#L48-L60),
[vxn2-clap/src/lib.rs:281](../../vxn-2/crates/vxn2-clap/src/lib.rs#L281)); a host
without the `timer-support` extension leaves the editor static — UI gestures
still reach the controller, but nothing echoes back. That is the documented
degraded mode, and it is exactly what shipped as a dead faceplate under
clap-wrapper's standalone in [E014](../../epics/closed/E014-standalone-builds.md).
clap-wrapper's AUv2 flavour has the same hole: `register_timer` is
`return false`
([auv2_base_classes.h:412](../../vendor/clap-wrapper/src/detail/auv2/auv2_base_classes.h#L412)).

AUv2 does, however, service `request_callback`
([auv2_base_classes.h:365](../../vendor/clap-wrapper/src/detail/auv2/auv2_base_classes.h#L365)),
drained at ~100 Hz from the macOS helper's own `CFRunLoopTimer`
([macos.mm:66](../../vendor/clap-wrapper/src/detail/os/macos.mm#L66)) into
`plugin->on_main_thread()`
([wrapasauv2.cpp:1187](../../vendor/clap-wrapper/src/wrapasauv2.cpp#L1187)).
So a plugin that re-arms `request_callback` from `on_main_thread` gets a tick
loop with no timer of its own. First ticket of
[E051](../../epics/open/E051-audio-unit-distribution.md); 0368-0370 put the
products on it.

## Design

One type in `crates/vxn-core-clap/src/gui.rs`, beside `WEBVIEW_TIMER_PERIOD_MS`:

```rust
pub enum TickSource {
    /// Host granted `timer-support`. Ticks arrive as `on_timer`.
    Host(HostTimer, TimerId),
    /// No timer, but the host services `request_callback`. Ticks arrive as
    /// `on_main_thread`, and each one re-arms the next.
    MainThreadCallback,
    /// Neither. The editor is static — today's behaviour, now named.
    None,
}
```

- `start(host) -> TickSource` — try `HostTimer::register_timer` at
  `WEBVIEW_TIMER_PERIOD_MS`; on absence or failure fall back to
  `MainThreadCallback` (arming it with one `host.request_callback()`);
  else `None`.
- `rearm(&mut self, host)` — no-op for `Host`/`None`, one
  `request_callback()` for the fallback. Called at the tail of
  `on_main_thread`.
- `stop(&mut self, host)` — unregisters the host timer, or drops the fallback
  to `None` so the loop stops. Called from `gui::destroy`.

`rearm` is deliberately the caller's job rather than something `start` can
guarantee: only the plugin knows whether the editor is still open, and a
fallback that re-arms after teardown is a callback loop that never ends.

No `unsafe`, no `CFRunLoopTimer` of our own, nothing macOS-specific — a
Windows or Linux host with the same gap gets the same fallback.

## Acceptance criteria

- [ ] `TickSource` exists in `vxn-core-clap::gui` with `start`/`rearm`/`stop`,
      documented with the AUv2 lineage above.
- [ ] Unit tests over a `clack-host` fake: a host offering `timer-support`
      yields `Host` and never calls `request_callback`; a host offering only
      `request_callback` yields `MainThreadCallback` and `rearm` calls it once
      per tick; a host offering neither yields `None` and `rearm` is inert.
- [ ] `stop` on the fallback arm leaves a subsequent `rearm` inert — the loop
      cannot outlive the editor.
- [ ] `cargo test -p vxn-core-clap` green; no product wired yet (0368-0370).

## Notes

- **Verify before building** (E051 Phase 0): `os::attach` adds the helper timer
  to `CFRunLoopGetCurrent()`
  ([macos.mm:82](../../vendor/clap-wrapper/src/detail/os/macos.mm#L82)). If a
  host instantiates the AU off the main thread, that run loop never spins,
  `onIdle` never fires and this fallback is dead. Contingency is a one-line
  patch of the vendored `macos.mm` to `CFRunLoopGetMain()`; it is upstreamable
  and should be offered upstream if we take it.
- Nothing in vxn implements `PluginMainThread::on_main_thread` today, so the
  hook is unclaimed.
- Related: [[vxn-mvc-model-authority]] — the tick is the Model → View pump, so
  losing it costs echo, never authority. That is why the `None` arm is a
  degraded mode and not an error.
- Out of scope: reviving the standalone. This removes one of E014's two
  blockers; the RtAudio TCC crash is untouched.
