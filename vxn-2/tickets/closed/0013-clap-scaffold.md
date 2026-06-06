---
id: "0013"
title: vxn2-clap crate scaffold + plugin entry + ports
priority: high
created: 2026-06-05
epic: E002
---

## Summary

Stand up the `vxn2-clap` crate as a `clack`-based CLAP plugin: workspace
member, `cdylib` + `rlib` crate types, plugin descriptor, extension
registration, audio + note port declarations, an empty `process()` that
returns silence. Downstream tickets attach params, events and rendering to
this skeleton.

Structurally mirrors `vxn-1/crates/vxn-clap` — copy the **shape**
(descriptor, `Plugin` impl, `Shared` / `MainThread` / `AudioProcessor`
split, port writers, `ScopedFlushToZero`, scratch buffer sizing) without
the `Controller` / `ViewEvent` / GUI / timer machinery, which belong to
the UI epic.

## Acceptance criteria

- [ ] `vxn2-clap` added to `vxn-2/Cargo.toml` workspace members.
- [ ] `crate-type = ["cdylib", "rlib"]` so the cdylib is host-loadable
      and integration tests can link the rlib (0020).
- [ ] Workspace `Cargo.toml` gets `clack-plugin`, `clack-extensions`
      (features: `audio-ports`, `note-ports`, `params`, `state`,
      `clack-plugin`), and a dev-dep `clack-host` — all pinned to the
      same git rev VXN1 uses to keep both workspaces in sync.
- [ ] Plugin descriptor: id `labs.vulpus.vxn2`, name `VXN2`, features
      `SYNTHESIZER | INSTRUMENT | STEREO`.
- [ ] `VxnPlugin` registers `PluginAudioPorts`, `PluginNotePorts`,
      `PluginParams`, `PluginState` via `declare_extensions`. (GUI +
      timer deliberately omitted — UI epic.)
- [ ] One stereo output audio port (`main`, channel_count 2,
      `IS_MAIN`, `STEREO` type). No input ports.
- [ ] One note input port supporting `CLAP | MIDI` dialects, preferred
      `Clap`. No note output.
- [ ] `Shared` holds `Arc<SharedParams>` (placeholder; 0014 fills it
      with the real type). `MainThread` holds the shared ref only.
      `AudioProcessor::activate` constructs a `vxn2_engine::Synth` from
      `audio_config.sample_rate` and allocates `scratch_l` / `scratch_r`
      sized to `max_frames_count`.
- [ ] `process()` is a stub returning `ProcessStatus::Continue` after
      zeroing the host's output channels for the block. No events
      handled yet; no engine drive yet. Subsequent tickets fill it in.
- [ ] `ScopedFlushToZero` guard at the top of `process()` (re-export
      from `vxn2-engine`; copy the VXN1 impl if not present yet).
- [ ] `cargo build -p vxn2-clap` succeeds. `cargo build --release -p
      vxn2-clap` produces `libvxn2_clap.dylib` under `target/release/`.
- [ ] `clack_export_entry!(SinglePluginEntry<VxnPlugin>)` at the bottom
      of `lib.rs` so the cdylib exports the CLAP entry symbol.

## Notes

The cdylib + rlib split costs a small amount of build time but pays for
itself: 0020's integration test links the plugin via the rlib instead of
`dlopen`ing the cdylib, and the test runner gets actual stack traces on
panic.

Pin `panic = "unwind"` already in `vxn-2/Cargo.toml` `[profile.release]`
— don't override it in `vxn2-clap`. Clack catches panics at the FFI
boundary so the host doesn't crash; an `abort` strategy would defeat
that.

VXN1's `vxn_engine::ScopedFlushToZero` zeroes the FP control word's
denormal flags for the scope of one `process()` call and restores on
drop. Filter / delay feedback paths in the engine rely on it to avoid
denormal-tail CPU spikes. If `vxn2-engine` doesn't yet expose one, the
cleanest move is to add it there (single struct + Drop impl) rather than
duplicating it in the CLAP crate.

No default-patch logic in this ticket — the engine ships its own
default in 0018; this crate just constructs `Synth::new(sample_rate)`.
