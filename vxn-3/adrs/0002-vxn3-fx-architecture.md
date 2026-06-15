# ADR 0002 — VXN3 FX architecture & routing

- **Status:** Accepted
- **Date:** 2026-06-15
- **Scope:** The effects system for VXN3 — the FX module roster, the routing
  topology (lane insert / send bus / master), internal-vs-external loops, and
  RT discipline. Extends and subsumes the dub-send sketch in
  [ADR 0001 §3](0001-vxn3-overall-design.md).

## Context

ADR 0001 §3 introduced dub sends (delay + reverb on global buses, p-lockable
send amount = the throw) and external send/return over CLAP audio ports. That
was a sketch. FX in VXN3 is bigger than two send buses: we want a palette of
processors usable at several scopes — sharpening a single drum, a shared dub
loop, or glue on the master. Dub *gating* turns out to be one use of a more
general per-lane send control. This ADR plans the whole system so the routing,
budgets, and RT cost are fixed up front rather than accreted.

## Decision

### 1. One FX module roster, placed in scoped slots

There is a single closed roster of FX modules. An **FX slot** is a polymorphic
slot holding one FX module instance — the same pattern as engine slots
(ADR 0001 §4): module swapped off the audio thread, dispatched per-block,
bypassed slots skip processing. Modules are **scope-agnostic**: the same module
can be instantiated in any slot. Three slot scopes:

1. **Lane insert** — in a track's own signal path. Per-lane timbre/dynamics.
2. **FX bus (send/return loop)** — a shared bus lanes send to. The bus is
   *either* an internal FX chain *or* routed externally over a CLAP
   send/return port pair (ADR 0001 §3). The "external stereo pairs" are simply
   buses configured external.
3. **Master insert** — the master chain.

### 2. FX roster

`Distortion`, `Phaser`, `Reverb`, `Delay`, `Bitcrush`, `Compressor`, `Limiter`,
`EQ`, `Gate`. `EQ` is a cheap biquad-based filter/EQ. `Gate` is a general-purpose
gate (usable as a noise/dynamics gate inserted anywhere, and for hard rhythmic
gating). No chorus (no need). Note dub *gating* does not require the `Gate`
module — p-locking a lane's send amount (ADR 0001 §3a) already gates a lane into
a bus; `Gate` is the sharper, insertable processor on top.

Placement is **orthogonal except for two dynamics modules**, which are scope-
restricted because they only make sense at a shared/summed stage:

| module         | lane insert | FX bus | master       |
| -------------- | ----------- | ------ | ------------ |
| **Compressor** | ✗           | ✓      | ✗            |
| **Limiter**    | ✗           | ✗      | ✓ (terminal) |

All other modules (Distortion, Phaser, Delay, Bitcrush, EQ, Gate, Reverb) are
free in any slot.

`Compressor` is **FX-bus only** — bus compression on a summed send is the useful
case; per-lane compression is not wanted. `Limiter` is **master only** and is
the fixed terminal stage of the master chain: it introduces lookahead latency,
which belongs on one global stage rather than replicated per-lane or per-bus
(where it would also stack delays). Wasteful placements (reverb on a lane
insert) are allowed and UI defaults steer time-based modules to buses, but
nothing else is hard-restricted.

### 3. Signal flow

```text
lane → [insert 1] → [insert 2] → gain/pan ─┬──────────────────────────→ main mix
                                           ├─[send A p-lock]→ FX bus A ─→ (int chain | ext A) → return → main mix
                                           ├─[send B p-lock]→ FX bus B ─→ (int chain | ext B) → return → main mix
                                           ├─[send C p-lock]→ FX bus C ─→ (int chain | ext C) → return → main mix
                                           └─[send D p-lock]→ FX bus D ─→ (int chain | ext D) → return → main mix
main mix → [master chain: EQ/filter → (inserts) → Limiter (terminal)] → out
```

### 4. Stereo everywhere

All FX modules and all slots are stereo (process L/R), one kernel per module.
Lane inserts run **post-pan** so the path is stereo end-to-end and FX authoring
is uniform. Cost: lane-insert processing is ~2× a mono path; accepted — VXN3
targets ~16 mono-ish lanes on Apple Silicon, well within the headroom shown by
VXN1/VXN2. No separate mono kernels to maintain.

### 5. Fixed slot budget

Preallocated, so worst-case CPU is bounded and known:

- **2 insert slots per lane** (a short serial chain).
- **4 FX buses.** Each bus is internal-chain *or* external; 2 external
  send/return pairs exist (ADR 0001 §3), so up to 2 buses can be external at
  once.
- **Master chain:** EQ/filter → optional master insert(s) → `Limiter`
  (fixed terminal). No compressor here (bus-only). The limiter's lookahead
  latency is the plugin's reported latency (CLAP `latency` extension / PDC).

### 6. FX parameter automation: send amounts only in v1

- **Per-lane send amount is p-lockable** (ADR 0001 §3a) — full step/ramp/latch
  behaviour. This is what makes dub throws *and* dub gating sequenceable.
- **Bus and master FX params are global** (UI knobs) for v1 — they are not
  per-lane, so they have no lane to carry their locks. Per-bus/master FX
  automation via dedicated control lanes is deferred.
- **Lane-insert FX params** are per-lane and lockable via that lane's locks
  like any other track param.

## Consequences

- FX and engines share one slot/dispatch/swap discipline — build it once.
- Worst-case FX cost is fixed: 16 lanes × 2 stereo inserts + 4 buses + master.
  Validate against the RT budget before committing kernels.
- The CLAP `audio-ports` layout (ADR 0001 §3) must expose exactly the external
  pairs the bus count allows (2).
- Dub gating needs no new DSP — it is p-locked send automation.

## Alternatives considered

- **Typed placement (sends vs inserts as different module sets):** less code
  reuse, more concepts; rejected for mostly-orthogonal placement (one roster,
  two scope-restricted dynamics modules — Compressor bus-only, Limiter
  master-only).
- **Mono lane inserts pre-pan:** cheaper CPU but two kernels per module;
  rejected for uniform-stereo simplicity given the CPU headroom.
- **Omitting a gate module:** considered, since p-locked send amount covers the
  dub-gate case — but a gate is generally useful as an insertable dynamics/noise
  gate and for sharp rhythmic gating, so `Gate` is included in the roster.
- **Per-bus FX param control lanes in v1:** more power, more UI surface;
  deferred to keep v1 accessible.
