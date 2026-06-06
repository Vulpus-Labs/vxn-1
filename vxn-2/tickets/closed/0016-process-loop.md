---
id: "0016"
title: Audio process — notes, MIDI, transport, render
priority: high
created: 2026-06-05
epic: E002
---

## Summary

Fill `VxnAudioProcessor::process()`: drive the engine from input events
(CLAP note + param + raw MIDI), drive tempo from the host transport,
render stereo into scratch buffers, copy out. After this ticket the
plugin makes sound — a host can load it, play notes, and hear the
engine respond to its default per-param values (illustrative default
patch lands in 0018).

## Acceptance criteria

- [ ] At entry: `_ftz = ScopedFlushToZero::new()` (denormal guard for
      the block).
- [ ] At entry: `local.fetch_ui_changes(shared)` (no-op for E002) then
      `local.write_to(synth.params_mut())` so the engine sees the
      authoritative param set for the block.
- [ ] Host transport: when `process.transport` is `Some` and
      `TransportFlags::HAS_TEMPO` is set, call
      `synth.set_tempo(tempo as f32)` so LFO1 sync and delay sync
      track BPM changes within one block.
- [ ] Output port: take port 0, channels into `f32`, assert 1 or 2
      channels available. Bail with `PluginError::Message` if neither
      (matches VXN1 pattern).
- [ ] Sample-accurate event handling: iterate `events.input.batch()`,
      and for each batch's `[start, end)` sample range render the
      engine only inside that range (so a note-on at sample 100 in a
      256-sample block produces 156 samples of post-note audio, not
      256). Mirrors VXN1's per-batch render loop.
- [ ] Per-event dispatch (inside the batch loop):
      - `NoteOn` (CLAP): `synth.note_on(key as u8, velocity as f32)`
        when `Match::Specific`.
      - `NoteOff` (CLAP): `synth.note_off(key as u8)` likewise.
      - `ParamValue`: `local.apply_input(event)` → `synth.set_param`.
      - `Midi` raw:
        - `0xE0` pitch bend: 14-bit value → normalised −1..+1 →
          `synth.set_pitch_bend(norm)`.
        - `0xB0 d1==1` CC1 mod wheel: deadzone bottom 1 LSB, scale
          to 0..1, `synth.set_mod_wheel(value)`.
        - `0xD0` channel aftertouch: scale to 0..1,
          `synth.set_aftertouch(value)`.
        - Other status bytes: ignored.
      - Other CLAP event types: ignored (sentinel matches).
- [ ] Scratch render: `synth.process(&mut scratch_l[start..end],
      &mut scratch_r[start..end])` per batch range. Scratch buffers
      sized once in `activate()` from `max_frames_count`; never
      reallocate.
- [ ] Copy-out: copy `scratch_l` into channel 0, `scratch_r` into
      channel 1 (if present). Mono hosts (1-channel output) get
      `scratch_l` only — silently downmix is wrong here; document
      the choice (we drop the R side, expecting hosts to allocate
      stereo for an instrument port).
- [ ] At exit: `local.publish(shared)` and
      `local.emit(shared, events.output, frames as u32)` so host
      automation echoes back into the shared store and (eventually,
      via 0014's emit stub) the host sees gesture-bracketed UI
      writes.
- [ ] `reset()` calls `synth.reset()` so the engine clears voices,
      smoothers, delays, and reverb tails on host transport restart
      or plugin reset.
- [ ] Integration test: host sends note-on at sample 0 + note-off at
      sample 200 inside a 256-sample block; render produces a
      non-silent attack region and a release tail that decays
      smoothly across blocks. Output is finite, non-NaN.
- [ ] No allocation in `process()`. No `unwrap` / `expect`. The
      `into_f32()` and `output_port(0)` paths use
      `.ok_or(PluginError::Message(…))?`.

## Notes

The split between `local.write_to` (top of block) and
`local.apply_input` (per-event) is intentional: host automation
between blocks lands via the param event stream, but slow UI writes
land via `fetch_ui_changes` and need to be pushed to the engine before
the first sample of the block. For E002 there are no UI writes, but
the wiring stays so 0016's structure survives the UI epic untouched.

The per-batch `[start, end)` slice arithmetic is the VXN1 pattern —
copy it. Off-by-one bugs there cost real time in 2026-04; the bound
extraction (`Included` / `Excluded` / `Unbounded`) is fiddly but
correct in `vxn-clap`. Lift the helper unchanged.

Channel aftertouch is on the source list in ADR §6 / `PARAMETERS.md`
but missing in some sketch code. Make sure the engine exposes
`set_aftertouch(f32)` and the matrix `aftertouch` source reads from
it; cross-check 0008 if absent.

Mono fallback: if a host only allocates 1 channel and the patch has
stereo content, mixing L+R together is non-trivial (peaks). Just
emit L. If a user complains we revisit; for now an instrument port is
stereo per ADR §1 and §9, mono hosts are out-of-spec.

Pitch bend +/-: ADR §6 doesn't specify a bend range. VXN1 uses
±2 semitones by default with a non-automatable patch param to extend.
For E002 default is ±2 semitones, configurable only via engine const
for now; the patch-level bend range param lands with the UI epic.
