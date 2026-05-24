# ADR 0002 — VXN1 feature roadmap (post-v1)

- **Status:** Accepted
- **Date:** 2026-05-24
- **Scope:** The set of synthesis/performance features VXN1 will grow beyond
  the first draft (ADR 0001), and the deliberate points where we diverge from
  the Roland Jupiter-8 that inspired it.

This ADR records *what* we are adding and *why*, plus the divergences from the
JP-8 reference. It does not fix implementation detail (parameter ids, DSP
internals); narrower ADRs or the code can settle those as each feature lands.

## Context

ADR 0001 settled the architecture. The first draft is a clean two-oscillator
subtractive polysynth: 2 VCOs (sine/tri/saw/pulse) + noise → ladder VCF
(−12/−24 dB) → VCA, a 5×4 modulation matrix (ENV-1, ENV-2, LFO, velocity,
key-follow → pitch, cutoff, amp, PWM), one LFO, chorus and delay, 16-voice
poly, oversampling.

We reviewed the Jupiter-8 owner's manual to mine its panel for features worth
having. The goal is **not parity** — the JP-8 is a 1981 instrument with
hardware constraints we do not share. We cherry-pick what earns its keep and,
where our software freedom lets us do better, we deliberately diverge.

The current gaps (relative to the JP-8 and to player expectations) are: no
oscillator sync, no cross-modulation/FM, only plain poly voice assignment, no
glide, no high-pass filter, no LFO delay, and a single LFO. The engine already
has a pitch-bend hook with nothing mapped to it (ADR 0001, "Deferred").

## Decision

We will implement the following, in roughly this order of value. Each is gated
on not regressing the allocation-free, block-rate control model from ADR 0001.

### 1. Oscillator sync (hard sync)

VCO-2 phase resets to VCO-1's cycle. Reuses the existing per-voice phase state
in `vxn-dsp`. Classic aggressive lead/sweep timbre.

### 2. Cross-modulation / linear FM

VCO-2 output modulates VCO-1 pitch (the JP-8 "Cross Mod" slider). Metallic and
ring-mod-like tones; pairs naturally with sync. A depth parameter, audio-rate.

### 3. Unison assign mode

Voice stacking: all available voices driven from one key (with detune). High
value for fat leads and basses. Sits alongside the existing poly allocator as
an assign mode — see §10 for how this interacts with key modes.

### 4. Portamento (polyphonic glide)

Per-voice pitch glide from the previous note. Time parameter; on/off. The JP-8
offered Upper-only/Off/On — our split/dual handling (§10) generalises that.

### 5. Envelope time scaling by key

Shorten ADSR attack/decay/release as pitch rises, the way acoustic instruments
decay faster up high. This is **distinct** from the existing `Key→{pitch,
cutoff,amp,pwm}` matrix destinations — it scales envelope *times*, not a mod
depth. A per-envelope amount (the JP-8's per-envelope Key Follow switch,
generalised to a continuous control).

### 6. High-pass filter

A −6 dB/oct HPF in front of the VCF (the JP-8 topology: Source Mixer → HPF →
VCF → VCA). VXN1 currently has no high-pass at all; this is the cheapest way to
thin body and shape acoustic-style patches.

### 7. LFO delay / fade-in

LFO modulation fades in over a settable time after note start (JP-8 Delay Time,
0–4 s). Delayed vibrato is the canonical use.

### 8. Second routable LFO

Rather than copy the JP-8's "VCO-2 LowFreq" trick (turning osc2 into a crude
mod source — a hardware economy we don't need), we add a **second full LFO** as
a matrix source. This extends the modulation matrix from 5 to 6 sources.

### 9. MIDI bend / mod-wheel routing

Wire the deferred bend hook from ADR 0001:
- **Pitch bend → pitch only.** Deliberately not routable elsewhere; predictable.
- **Mod wheel → mappable to cutoff or osc2.** Routing the wheel to osc2 (pitch
  or sync depth) is specifically useful in the hard-sync case: the played pitch
  comes from osc1 while osc2 supplies the synced waveform, so sweeping osc2 with
  the wheel is the expressive gesture.

### 10. Key modes — Whole / Dual / Split

The JP-8's three key modes. Whole = one patch across the keyboard; Dual = two
patches layered; Split = two patches across a split point. This is the largest
item: it implies two patch layers, a split point, and per-layer routing of
hold/portamento/etc.

**UI:** in dual-layer modes the editor switches the *displayed* layer via
buttons (one faceplate, toggled between Upper/Lower), rather than showing two
faceplates at once. Detailed UI is left to a later ADR.

### 11. LFO host-sync and reset

- **Host sync (optional):** when enabled, the LFO rate control selects musical
  beat subdivisions (straight, dotted, triplet) locked to host tempo, instead
  of free Hz.
- **Reset on note start (optional):** retrigger LFO phase at note-on for
  repeatable per-note modulation shapes.

Both apply per LFO (§8 gives us two).

### Oscillator tuning model (applies throughout)

VXN1 keeps the JP-8's ability — which the JP-8 itself lacked — to set
oscillators at **non-octave intervals** (e.g. a fifth, 7 semitones). Tuning is
exposed as **three controls per oscillator: octave, semitones, cents**, rather
than the JP-8's octave-only footage switch. (The v1 model already has
coarse/fine in semitones+cents; this formalises an explicit octave control.)

## Explicitly not doing

- **Arpeggiator** — out of scope; sequencing belongs to the host/player.
- **Patch / tape memory, DCB, CV/Gate** — these are hardware patch-storage and
  analog-interfacing concerns; the plugin host owns preset state.
- **ENV-1 polarity invert switch** — a negative matrix depth already inverts an
  envelope's effect.
- **VCA discrete LFO steps (0/1/2/3)** — our continuous `LFO→Amp` matrix
  destination is strictly more capable.
- **VCO-2 LowFreq mode** — superseded by the second full LFO (§8).
- **Parity for its own sake** — anything not earning its keep is omitted.

## Consequences

- The modulation matrix grows from 5×4 to **6×4** (second LFO as a source).
  Source-major/dest-minor layout from ADR 0001 still holds; the matrix base and
  count shift.
- Key modes (§10) are the structural heavy lift: two patch layers touch the
  parameter model, voice allocation, the engine's per-block context, and the
  editor. It likely warrants its own ADR before implementation.
- Sync, cross-mod and FM add coupling between the two oscillators in the
  `vxn-dsp` poly oscillator hot path; care needed to keep it vectorised.
- New per-voice state (glide source pitch, LFO phase for reset, envelope
  time-scale factors) must stay in the structure-of-arrays voice bank and reset
  cleanly on `reset_all`.
- Host-synced LFO introduces a tempo/transport dependency into the engine,
  which has so far been transport-agnostic.

## References

- ADR 0001 — overall design (architecture, deferred items including the
  unmapped bend hook and the single-surface matrix view).
- Roland Jupiter-8 Owner's Manual (panel layout, key/assign modes,
  cross-mod, sync, portamento, LFO delay, envelope key-follow).
- Roadmap summary tracked in project memory.
