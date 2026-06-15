---
id: "0046"
product: vxn-3
title: "vxn-3 CLAP shell + crate skeleton + host transport sync (silent)"
priority: high
created: 2026-06-15
epic: E021
---

## Summary

Stand vxn-3 up as a loadable CLAP plugin that syncs to host transport but
renders silence. Establishes the crate skeleton and the shell reuse from
`vxn-core-*`, so every later slice has somewhere to land.

## Design

- **Crates.** Create `vxn-3/crates/vxn3-dsp`, `vxn3-engine`, `vxn3-clap`
  (mirroring the vxn-1/vxn-2 split). Uncomment the workspace member slots in
  the root `Cargo.toml` (placeholders added at scaffold).
- **CLAP shell.** Reuse `vxn-core-clap` / `vxn-core-app` for the plugin entry,
  factory, and host glue (clack). Stereo audio output port only for now (no
  send/return ports — those are 0051 / deferred).
- **Transport.** Read the CLAP transport from the process call: tempo (BPM),
  play/stop, song position (PPQ / beat time). Expose it to the engine layer as
  the clock the sequencer will consume in 0047/0048. No sequencer yet.
- **Process.** Allocation-free callback that writes silence; verify with the
  workspace's alloc-trap harness pattern.

## Acceptance criteria

- [ ] vxn-3 builds for the host platform and loads in a CLAP host
      (clap-validator / a DAW) without errors.
- [ ] The plugin reports a stereo output port and renders silence.
- [ ] Transport (tempo, play/stop, song position) is read each block and
      surfaced to the engine layer.
- [ ] Process callback is allocation-free (alloc-trap clean).
- [ ] Root `Cargo.toml` builds with the three new crates as members.

## Notes

- Out of scope: any sound, sequencer, or UI. This is the empty vessel.
- Design: `vxn-3/adrs/0001` (overall), `vxn-3/README.md`.

## Close-out (2026-06-15)

- Three crates created mirroring the vxn-1/vxn-2 split:
  [vxn3-dsp](../../vxn-3/crates/vxn3-dsp/src/lib.rs) (empty stub — kernels land
  in 0047/0049), [vxn3-engine](../../vxn-3/crates/vxn3-engine/src/engine.rs)
  (silent `Engine` + `Transport` clock), and
  [vxn3-clap](../../vxn-3/crates/vxn3-clap/src/lib.rs) (clack shell). Root
  `Cargo.toml` members + path deps uncommented; `cargo build --workspace` green.
- CLAP shell keeps its own `Plugin` impl and reuses `vxn-core-clap`
  (`tempo_from_transport`). Descriptor `labs.vulpus.vxn3` (INSTRUMENT/DRUM/
  STEREO); one 2-channel `IS_MAIN` STEREO output port via `PluginAudioPortsImpl`;
  no params/notes/GUI/state yet (deferred to later E021 slices).
- Transport read every block by
  [read_transport](../../vxn-3/crates/vxn3-clap/src/lib.rs#L94): tempo,
  `IS_PLAYING`, and `HAS_BEATS_TIMELINE`→`song_pos_beats` map into `Transport`,
  handed to `Engine::set_transport`. The clock the 0048 sequencer will consume.
- Process callback writes silence from pre-allocated scratch — allocation-free.
  Verified by a new `cfg(test)` thread-local counting allocator (no shared
  alloc-trap harness existed in the workspace before this): test
  `vxn3_clap::tests::silent_render_is_alloc_free_and_silent` counts 0 allocs over
  1 s of `process_block`.
- Loads in a CLAP host: in-process clack-host smoke
  ([smoke.rs](../../vxn-3/crates/vxn3-clap/tests/smoke.rs)) drives
  `init → activate(44.1k/256) → start_processing → process`; tests
  `entry_loads_and_describes_vxn3`, `renders_silence_without_transport`,
  `renders_silence_under_running_transport` pass. Release cdylib exports
  `_clap_entry`; `clap-validator validate` passes (see below).
