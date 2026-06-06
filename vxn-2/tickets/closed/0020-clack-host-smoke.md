---
id: "0020"
title: End-to-end smoke test via clack-host
priority: medium
created: 2026-06-05
epic: E002
---

## Summary

In-process integration test that loads the `vxn2-clap` plugin via
`clack-host`, sends notes + automation, renders audio, and asserts
the plugin is alive. This is the gate that closes E002: if the smoke
test passes, the kernel + CLAP shell + default patch + state are all
wired correctly.

Lives in `vxn2-clap/tests/smoke.rs`. Links the plugin via its `rlib`
target (set up in 0013) — no `dlopen`, real stack traces on panic.

## Acceptance criteria

- [ ] `vxn2-clap/tests/smoke.rs` boots a `clack-host` test host,
      instantiates `VxnPlugin` via the in-process entry point, and
      runs the standard CLAP lifecycle: `init` → `activate(sr=44100,
      max_frames=256)` → `start_processing`.
- [ ] Default-patch render test:
      - Send `NoteOn(60, 0.8)` at sample 0.
      - Render 1 second (≈ 172 blocks of 256 samples).
      - Send `NoteOff(60)` at sample 0 of the 1-second mark.
      - Render 4 more seconds.
      - Assert: every sample is finite (`is_finite()`).
      - Assert: RMS over `[0.1 s, 0.9 s]` is in [−24, −9] dBFS.
      - Assert: RMS over `[4.5 s, 4.99 s]` is ≤ −60 dBFS (tail
        decayed).
- [ ] Parameter-sweep test:
      - Hold `NoteOn(64, 0.7)` for the duration of the sweep.
      - For each CLAP id in `0..TOTAL_PARAMS`:
        - Set param to `desc.min`. Render 128 samples. Assert
          finite.
        - Set param to `(desc.min + desc.max) / 2`. Render 128
          samples. Assert finite.
        - Set param to `desc.max`. Render 128 samples. Assert
          finite.
      - Total render at 44.1 kHz / 174 params × 3 settings × 128
        samples ≈ 1.5 s of audio. Fast enough to run on every PR.
- [ ] State round-trip test:
      - Set 10 params to non-default values via host events.
      - Render a few blocks (let `publish` propagate).
      - Call `state.save` into a `Vec<u8>`.
      - Instantiate a fresh plugin.
      - Call `state.load` with the saved blob.
      - Assert every param's `get_value` matches the source plugin's
        post-save values.
- [ ] Tempo test:
      - Set host transport tempo to 90 BPM, render a block.
      - Switch to 180 BPM, render another block.
      - Assert no `is_finite()` violation across the boundary.
      - (Audible verification of sync correctness is a manual test;
        this just guards against panics + NaN on transport edits.)
- [ ] Pitch-bend / mod-wheel test: send raw MIDI pitch bend at
      maximum + minimum (full ±2 st sweep) and CC1 0 → 127, render
      across; assert finite output throughout.
- [ ] Test runs under `cargo test -p vxn2-clap` with no special
      setup. CI-ready.

## Notes

`clack-host`'s pinning revision must match `clack-plugin`'s, or the
ABI shifts and the test segfaults at module load. The workspace dep
declaration from 0013 keeps both at the same `rev` — if they ever
drift, the smoke test is the canary.

The RMS thresholds in the default-patch test depend on the values
chosen in 0018. If 0018's listening-test tuning shifts the patch's
loudness materially, update the lower / upper bounds here. Better
than skipping the assertion: a too-quiet default patch is a real
regression.

The parameter-sweep test is the cheapest possible "every param at
extremes doesn't crash" guard. It will not catch *audio* bugs (a
filter that quietly NaN-tails over 10 seconds escapes), but it
catches the obvious classes (denormal explosion under extreme
detune, panics in a stepped-param boundary, etc.). VXN1 ships an
equivalent harness — copy the structure.

If `clack-host` ergonomics get in the way, the fallback is to drive
the plugin via `VxnAudioProcessor::process` directly from the test,
synthesising `Audio` / `Events` / `Process` arguments by hand. Less
fidelity (no `state` extension round-trip via the host harness, no
real CLAP wire-up) but faster to write. Try `clack-host` first.
