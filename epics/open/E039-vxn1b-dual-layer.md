---
id: E039
product: vxn-1b
title: "VXN1b dual-layer — two independent synths, per-layer matrix, 3-tab UI"
status: open
created: 2026-07-31
---

## Goal

Reintroduce VXN1's keyboard **split / dual-layer** capability into VXN1b as
**two fully independent synth instances**, each with its own patch, voice pool,
and private mod matrix — per [ADR 0002](../../vxn-1b/adrs/0002-vxn1b-dual-layer.md).

When this epic closes:

- The plugin holds **2 × `Synth` + a global block**; a MIDI demux routes events
  (single / dual / split) with **route-on / broadcast-off** semantics.
- Each layer owns a **private 16-slot matrix**; all sources (incl. **LFO2**) are
  per-layer. The only cross-layer link is **LFO2→LFO2 rate+phase sync**.
- Global **drift** ports from VXN1 unchanged (one control, both synths).
- FX is **one global instance**; both synths mix in via layer balance.
- The UI is **three tabs** — Layer 1, Layer 2 (on/off), FX/Mixer/Global.
- Preset + host-state format carries two layers + KeyMode/split (versioned).

## Why now

Split/dual-layer is one of VXN1's strongest features. VXN1b dropped it (ADR 0001)
for simplicity; reinstating it — now over a matrix surface rather than fixed
panels — makes VXN1b a substantially more capable instrument. Doing it **before**
E038's UI is built avoids throwaway single-patch faceplate work.

## Relationship to E038 (important)

[[E038]] is **open and unbuilt** (0209–0213). Its 0209/0210/0211 build a
*single-patch* 3-row faceplate, a *single* matrix overlay, and an FX-tab panel —
all of which this epic reshapes into the 3-tab / per-layer structure. **E039's UI
tickets supersede E038's UI tickets**; E039 should land first (or absorb them).
E038's preset/release tickets (0212/0213) rebase onto the two-layer state format
delivered here. Resolve the overlap before starting E038 UI work.

## Design (locked by ADR 0002)

- **Two instances.** Core synth (voices + patch + matrix + drift consumers) →
  instantiable `Synth`; plugin holds 2 + global. Allocation/stealing/unison are
  per-synth. **16 voices/synth (32 max)**; synth 2 bypassed in single mode.
- **Demux.** single → synth 1 only; dual → fan to both; split → note-ons by
  pitch vs split point, other events to both. **Note-offs always broadcast.**
- **KeyMode.** Derived: L2 off = Single; L2 on = Dual, or Split if split enabled.
  KeyMode + split point are non-automatable blob state.
- **Per-layer matrix.** 16 slots each; topology blob ×2; 32 automatable depth
  params. No cross-layer routing.
- **LFO2 sync.** L2 LFO2 slaves to L1 (master) — rate + phase lock.
- **Drift.** Single global `MasterDrift`, ported verbatim from VXN1.
- **UI.** 3 tabs; matrix overlay per layer tab; global/mixer on Tab 3.

## Planned tickets

Chain: **0214 → 0215** (engine: instance + demux), then **0216/0217/0218**
(matrix ×2, LFO2 sync, drift) in parallel, then **0219 → 0220** (UI), then
**0221** (state/preset). 0221 can start once 0214 lands.

- [ ] **0214** — `Synth` as an instantiable unit. Wrap voices + patch + matrix +
      drift consumers; plugin holds 2 × `Synth` + global block. Per-synth voice
      pool/allocator/stealing/unison; 16 voices/synth. Single mode bypasses
      synth 2 (zero cost). Engine tests: two instances render independently.
- [ ] **0215** — MIDI demux + KeyMode. Front-end routing: single/dual/split;
      **route-on / broadcast-off**; KeyMode derived from L2-on + split-enable;
      split point + KeyMode as blob state. Test the split-move stuck-note case
      (broadcast-off resolves it).
- [ ] **0216** — Per-layer mod matrix. Second matrix instance; topology blob ×2;
      **32** `MatrixSlotNDepth` params (16/layer); sources all per-layer. Matrix
      eval private to each synth. Param-table + round-trip tests.
- [ ] **0217** — LFO2→LFO2 sync. L2 LFO2 slaves to L1 as master (rate + phase
      lock, mirror phase accumulator); sync flag (blob or param). Test phase
      lock + free-run fallback.
- [ ] **0218** — Global drift port. `MasterDrift` [0,1] param + per-voice random
      walk (osc pitch) + static trim draws (env/filter), scaled by drift; applied
      to both synths. Port from [vxn-engine/src/voice.rs](../../vxn-1/crates/vxn-engine/src/voice.rs).
- [ ] **0219** — 3-tab UI shell + Layer 1 / Layer 2 tabs. Tab strip; Layer tabs
      carry the full synth patch surface + per-layer matrix overlay; Layer 2
      on/off toggle drives KeyMode. Supersedes 0209/0210's single-patch layout.
- [ ] **0220** — FX / Mixer / Global tab. Layer balance → global FX; split enable
      + point; all FX params; master level/pitch/drift/limiting; LFO2-sync
      control. Supersedes 0211's FX-tab panel.
- [ ] **0221** — Two-layer state + preset format. Versioned host-state + preset
      TOML carrying both layers + KeyMode/split; migration for any single-patch
      VXN1b presets. Round-trip tests.

## Risks

- **CPU in dual/split.** ~2× voice cost. Mitigated: opt-in, synth 2 bypassed in
  single; render loop has headroom ([[vxn1-render-loop-optimized]]). Re-profile
  at full 32-voice dual before close.
- **State migration.** Format changes; any saved single-patch VXN1b presets need
  a version bump. Land 0221's migration before shipping.
- **E038 overlap.** Building E038's single-patch UI first is throwaway — resolve
  sequencing (see above) before either epic's UI work starts.
- **MVC discipline.** Two matrix overlays double the most-stateful view; keep the
  view-never-reads-model parity test per layer ([[vxn2-mvc-discipline]]).

## Acceptance

- Plugin runs 2 independent synths; single mode is byte-for-byte today's behavior
  with synth 2 bypassed.
- Split mode routes on pitch; moving the split point with held notes leaves **no
  stuck notes** (broadcast-off).
- Each layer has a private, independently programmable 16-slot matrix; LFO2 sync
  locks L2 to L1 in rate + phase.
- Global drift affects both layers from one control, matching VXN1's feel.
- 3-tab UI: Layer 1 / Layer 2 (on/off) / FX-Mixer-Global; FX is one global chain.
- Presets/host state round-trip both layers + KeyMode/split; migration covers
  pre-existing single-patch presets. DAW smoke clean (`verify-audio-in-reaper`).
