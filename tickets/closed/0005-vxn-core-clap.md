---
id: "0005"
title: vxn-core-clap — clack scaffold, event dispatch, state I/O
priority: high
created: 2026-06-06
epic: E001
---

## Summary

Extract the shared `clack` plugin scaffold currently duplicated
between vxn-1's `vxn-clap` (~600 LOC) and vxn-2's `vxn2-clap`
(~1100 LOC). The three-thread split (Shared / MainThread /
AudioProcessor), CLAP event dispatch, `LocalParams` audio-thread
mirror, state blob save/load, and gesture-bracket emit are all
synth-agnostic given a generic engine that satisfies a small set of
traits.

## Acceptance criteria

- [ ] `vxn_core_clap::engine` traits — minimum surface the shell
      needs from a synth engine:
      - `EngineProcess`: `process_block(left: &mut [f32], right: &mut [f32])`,
        `reset()`, `set_sample_rate(f32)`, `set_block_size(usize)`.
      - `EngineNotes`: `note_on(key: u8, vel: f32)`,
        `note_off(key: u8)`, `pitch_bend(value: f32)`,
        `mod_wheel(value: f32)`, `aftertouch(value: f32)`.
      - `EngineParams`: a `ParamModel`-backed view the shell pokes
        on flush. Re-exports `vxn_core_app::ParamModel`.
- [ ] `vxn_core_clap::plugin::SynthPlugin<E>` — generic over an
      `E: EngineProcess + EngineNotes + EngineParams + Send + 'static`.
      Implements `clack_plugin::Plugin` with the Shared / MainThread /
      AudioProcessor split. Each synth instantiates with its own
      engine, descriptor, feature list.
- [ ] `vxn_core_clap::descriptor::SynthDescriptor` — builder for the
      CLAP descriptor (id, name, vendor, url, version, feature list).
      Synth supplies one of these at registration time.
- [ ] `vxn_core_clap::events::dispatch_event(event, engine)` —
      maps `clack` `InputEvent::Note*`, `ParamValue`, `ParamMod`,
      raw MIDI (pitch bend 0xE0, CC1 mod wheel, channel pressure
      0xD0) onto the engine traits. Pure function, no state.
- [ ] `vxn_core_clap::local::LocalParams` — per-block audio-thread
      param mirror. Folds inbound CLAP automation into a shadow
      array, marks dirty params, publishes back to a
      `SharedParams`-shaped store the engine reads. Generic over
      `N: usize` param count (or a `&[AtomicF32]` slice) — the
      synth supplies the storage.
- [ ] `vxn_core_clap::state` — save/load helpers. `save(model,
      header) -> Vec<u8>` writes a versioned header + flat f32
      array. `load(blob, model) -> Result<(), ParamLoadError>`
      verifies header, restores params via `ParamModel::set`.
      Identical wire format to vxn-1's current blob so existing
      vxn-1 patches survive the migration.
- [ ] `vxn_core_clap::gesture` — outbound gesture-bracket emit
      helpers. `begin(id)`, `end(id)`, `value(id, v)` enqueue
      `OutputEvents::Param*` writes.
- [ ] `vxn_core_clap::transport::tempo_from_transport(t)` — extract
      BPM from a CLAP transport event for the engine's host-sync
      consumers. Returns `Option<f64>`.
- [ ] Crate exposes a `clack_export_entry!` re-export so synths
      write one line at the cdylib root.
- [ ] Test suite (lifted + generalised from vxn-2's existing 35+
      tests in `vxn2-clap/lib.rs`): event dispatch round-trips note
      on/off + bend + CC; param round-trip via state save/load;
      block render against a stub engine; reset clears engine state;
      transport tempo propagates.
- [ ] No deps on `vxn-1` or `vxn-2`. Depends on `vxn-core-app`,
      `clack-plugin`, `clack-extensions` (audio-ports, note-ports,
      params, state, optionally gui — gui registration happens in
      each synth's shell when it adds an editor).

## Notes

`gui` extension registration is NOT in scope. Each synth keeps its
own `gui_create` impl that mounts its `WebEditor` (from
`vxn-core-ui-web`) — too much synth-specific window-handle wiring to
generalise cheaply. Cross-cutting only if a third synth shows up.

The state blob format must stay wire-compatible with vxn-1's
existing format. vxn-1 patches saved before the migration must
load cleanly after. Spell out the header byte layout in a
`// Wire format:` comment block.

vxn-2's `LocalParams` (250 LOC) is the more recent and cleaner of
the two. Use it as the source of truth; reconcile against vxn-1's
behaviour in the test suite, not by copying vxn-1's struct shape.

Generalising `SynthPlugin<E>` over the engine traits is the riskiest
type-level work in this epic. If `E` ends up needing
`for<'a> &'a mut E: Send` or similar `Send` gymnastics that aren't
yet satisfied, fall back to a trait-object boundary
(`Box<dyn EnginePlug + Send>`) and revisit. Don't burn this ticket
on type-system battles — ship a working version first.
