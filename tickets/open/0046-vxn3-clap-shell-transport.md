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

- [x] vxn-3 builds for the host platform and loads in a CLAP host
      (clap-validator / a DAW) without errors. *(In-process clack-host smoke
      test drives the full `init → activate → start → process` lifecycle and
      asserts the entry/factory describe `labs.vulpus.vxn3`; release cdylib
      exports `_clap_entry`. clap-validator not installed — DAW load is the
      remaining manual check.)*
- [x] The plugin reports a stereo output port and renders silence. *(Audio-port
      ext declares one 2-channel `IS_MAIN` STEREO output; smoke test renders 8
      blocks of finite zeros.)*
- [x] Transport (tempo, play/stop, song position) is read each block and
      surfaced to the engine layer. *(`read_transport` maps tempo / `IS_PLAYING`
      / `HAS_BEATS_TIMELINE`→`song_pos_beats` into `Transport`, handed to
      `Engine::set_transport` every block.)*
- [x] Process callback is allocation-free (alloc-trap clean). *(0046 introduces
      a `cfg(test)` thread-local counting allocator; `process_block` over 1 s of
      blocks counts 0 allocations.)*
- [x] Root `Cargo.toml` builds with the three new crates as members. *(`cargo
      build --workspace` green; members + path deps uncommented.)*

## Notes

- Out of scope: any sound, sequencer, or UI. This is the empty vessel.
- Design: `vxn-3/adrs/0001` (overall), `vxn-3/README.md`.
