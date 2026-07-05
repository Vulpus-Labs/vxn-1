# VXN2 — Developer's Manual

A working guide to the VXN2 source tree: what the crates are, how audio flows
from a MIDI note to the output buffer, how the UI talks to the engine, and the
implementation choices worth knowing before you touch anything. Written for
someone who has never read this code.

VXN2 is a **6-operator FM synthesizer** shipped as a Rust **CLAP** plugin, in
the DX7/Montage lineage, extended with first-class **voice stacking** (unison
"supersaw" without wavetable storage). It is a sibling instrument to VXN1 (a
subtractive polysynth); the two share the CLAP shell, preset conventions and
the HTML-faceplate UI idiom, but have **separate DSP kernels, parameter tables
and patch formats**.

The canonical design record is [`adrs/`](adrs/) + [`PARAMETERS.md`](PARAMETERS.md);
this manual summarises and cross-links them, but the ADRs win on any conflict.

---

## 1. Repository layout

Everything lives in one Cargo workspace at the repo root (`vxn-1/` and `vxn-2/`
are *not* separate workspaces — that was an early plan that never happened).
The VXN2 crates are:

| Crate | LOC | Role |
|-------|-----|------|
| [`vxn2-dsp`](crates/vxn2-dsp) | ~10k | Pure DSP kernels — operators, algorithms, voices/stacks, envelopes, LFOs, filter, FX. No framework, no allocation on the hot path. |
| [`vxn2-engine`](crates/vxn2-engine) | ~12k | The synth engine — voice allocation, mod matrix, block render loop, param table, preset I/O, shared state. |
| [`vxn2-clap`](crates/vxn2-clap) | ~2k | CLAP plugin shell via `clack` — process callback, host param/note events, state save/load, GUI extension. |
| [`vxn2-app`](crates/vxn2-app) | ~0.4k | MVC controller glue — translates UI intents into model writes (VXN2-specific custom events). |
| [`vxn2-ui-web`](crates/vxn2-ui-web) | ~1.2k | Embeds and serves the HTML/JS/CSS faceplate; defines the UI↔engine JSON message protocol. |
| [`vxn2-osc-bench`](crates/vxn2-osc-bench) | — | Criterion benches for every hot kernel. |

Dependency direction: `ui-web → app → engine → dsp`, and `clap` sits on top
tying `engine` + `app` + `ui-web` together. Shared primitives (CLAP helpers,
preset-browser JS, smoothing/math utils) come from root `vxn-core-*` crates.

The mental model in one line:

```
MIDI/host params ─► vxn2-clap (RT thread) ─► vxn2-engine.process_block ─► vxn2-dsp kernels ─► audio out
                                    ▲                                                     │
      HTML faceplate ◄─ vxn2-ui-web ◄─ vxn2-app controller ◄── dirty-bitset pump ◄────────┘ (main thread)
```

---

## 2. The DSP core (`vxn2-dsp`)

This crate is framework-free number-crunching. It has **two parallel paths**:

- a **scalar reference voice** ([`voice.rs`](crates/vxn2-dsp/src/voice.rs)) — a
  single-lane oracle used by tests and benches; and
- the **production SoA stack** ([`stack.rs`](crates/vxn2-dsp/src/stack.rs)) — an
  8-lane lane-packed voice that is what actually renders in the plugin.

They share the operator, algorithm and envelope code, and are cross-checked
against each other in tests.

### 2.1 The operator ([`op.rs`](crates/vxn2-dsp/src/op.rs))

An operator is a **Q32 fixed-point phase accumulator + sine + 4-rate/4-level
DX7 envelope**. Phase is a `u32` where `2^32 == one cycle`, so wraparound is
free (`wrapping_add`) and there is zero long-term drift. Everything past the
phase boundary is `f32`.

The per-sample tick is branch-free and about 15 cycles:

```rust
// op.rs — the hot path
#[inline(always)]
pub fn op_tick(state: &mut OpState, mod_in: f32) -> f32 {
    let pm_q32 = (mod_in * PM_SCALE_Q32) as i32 as u32;   // modulation in cycles
    let phase_mod = state.phase.wrapping_add(pm_q32);
    let out = sine::scalar::fast_sine_q32(phase_mod) * state.eg.level;
    state.fb_prev2 = state.fb_prev1;                        // 2-sample feedback delay
    state.fb_prev1 = out;
    state.phase = state.phase.wrapping_add(state.phase_inc);
    out
}
```

Key conventions:

- **Modulation is in cycles.** `PM_SCALE_Q32 = 2^32`, so a modulator output of
  `1.0` means one full cycle of phase shift. This unifies FM depth across the
  whole engine.
- **Feedback uses a 2-sample average** (`0.5·(prev1+prev2)`), the DX7
  anti-aliasing convention. The router does *not* inject feedback; the
  voice/stack loop does, just before ticking (§2.3).
- **EG level is held constant across the tick.** Envelopes advance separately
  at *control rate* (once per block), not per sample.

### 2.2 The sine ([`sine.rs`](crates/vxn2-dsp/src/sine.rs))

Sines are **approximated, not table-looked-up** — a Bhaskara+Moser polynomial,
branch-free, ~−59 dB THD:

```rust
// sine.rs
#[inline(always)]
pub fn fast_sine_01(p: f32) -> f32 {
    let x1 = p - 0.5;
    let x2 = x1 * 16.0 * (x1.abs() - 0.5);
    x2 + 0.225 * x2 * (x2.abs() - 1.0)
}
```

Rationale (from the README premises): 5 mul + 2 abs + 2 add, vectorises
beautifully, and avoids the SIMD-hostile gather that a table lookup needs. THD
is masked under hypersaw detune anyway. The scalar polynomial is the only
reader: LLVM auto-vectorises it across the 8-lane SoA loop, so there is no
hand-written NEON path, and the residual THD never warranted a higher-fidelity
table variant.

### 2.3 Algorithms ([`algo.rs`](crates/vxn2-dsp/src/algo.rs))

The 32 canonical DX7 algorithms are a static table. Each `AlgoSpec` holds the
modulator→carrier edges, a carrier bitmask, and exactly one feedback pair
(`fb_src → fb_dst` — a self-loop for 30 algorithms, a two-op loop for algos 4
and 6, matching the DX7 chart). There is **no per-op feedback** parameter; a
single global feedback control scales the one path. Arbitrary cross-op PM is out
of scope by design — the mod matrix routes control-rate modulation (level, pitch,
pan, phase), not audio-rate operator-into-operator carriers, and PM topology is
fixed by the selected algorithm, not patched per voice.

Two load-bearing tricks:

- **One-sample-delay routing.** Every modulation edge consumes the *previous*
  sample's op outputs. This removes within-sample data dependencies so the ops
  can be evaluated "in parallel" — which is what makes the SoA lane packing
  vectorise.
- **Each algorithm is its own function symbol.** A macro unrolls the edge list
  into straight-line adds, and `#[inline(never)]` keeps each algo a visible
  symbol so an `objdump` in code review can *verify* the codegen is branch-free
  (a hard-won VXN1 lesson: a runtime `match` in the poly loop silently drops
  NEON to scalar).

```rust
// algo.rs — one algorithm compiled to a distinct, branch-free symbol
macro_rules! impl_route {
    ($name:ident, edges = [$(($m:literal, $c:literal)),*], carriers = [$($cs:literal),*]) => {
        #[inline(never)]
        fn $name(prev: &[f32; N_OPS]) -> ([f32; N_OPS], f32) {
            let mut mi = [0.0_f32; N_OPS];
            $( mi[$c - 1] += prev[$m - 1]; )*
            let carrier_sum = 0.0_f32 $( + prev[$cs - 1] )*;
            (mi, carrier_sum)
        }
    };
}
```

`algo.rs` also owns `pitch_stack_component(algo, wall_mask, target_op)`, a pure
`const`-style graph flood-fill used by stack-pitch modulation (§2.4, ADR 0005).

### 2.4 Voices and stacks — the SoA hot loop ([`stack.rs`](crates/vxn2-dsp/src/stack.rs))

A "voice" in VXN2 is a **stack**: up to `STACK_LANES = 8` concurrent operator
instances, detuned/decorrelated, that together form one played note's unison.
The struct is **Structure-of-Arrays**: for each of the 6 ops, `phase`,
`phase_inc`, feedback history, EG level etc. are `[_; 8]` arrays laid out
contiguously so the inner loop autovectorises with no per-lane branches.

The stack is split into three sub-structs for cache coherence
(`StackCore` = hot DSP data, `StackMeta` = control-rate note state,
`StackModulation` = block-rate matrix scratch).

The kernel — `tick_ops` — is the single hottest function in the plugin. Note
how feedback is injected once (hoisted out of the op loop), and the per-op work
is three straight-line lane loops:

```rust
// stack.rs — the 8-lane kernel (abridged)
fn tick_ops(stack: &mut Stack) -> [[f32; STACK_LANES]; N_OPS] {
    let (mut mi, _cs) = (stack.meta.route_fn)(&stack.core.prev_outs);
    let spec = spec_of(stack.meta.algo);
    let (fs, fd) = ((spec.fb_src - 1) as usize, (spec.fb_dst - 1) as usize);
    {   // per-lane feedback injection, hoisted out of the op loop
        let src = &stack.core.ops[fs];
        let (p1, p2, fbs) = (src.fb_prev1, src.fb_prev2, src.fb_scale);
        for k in 0..STACK_LANES {
            mi[fd][k] += 0.5 * (p1[k] + p2[k]) * fbs[k];
        }
    }
    let mut new_outs = [[0.0_f32; STACK_LANES]; N_OPS];
    for i in 0..N_OPS {
        let lvl = stack.core.op_eg_level[i];       // contiguous EG mirror (see below)
        let lvl_mod = stack.core.op_level_mod[i];
        let fade = stack.core.op_nyquist_fade[i];
        let op = &mut stack.core.ops[i];
        for k in 0..STACK_LANES {
            let phase_mod = op.phase[k]
                .wrapping_add((mi[i][k] * PM_SCALE_Q32) as i32 as u32)
                .wrapping_add(stack.core.op_phase_mod_q32[i][k]);
            let s = fast_sine_q32(phase_mod) * ((lvl[k] + lvl_mod[k]) * fade[k]);
            new_outs[i][k] = s;
            op.phase[k] = op.phase[k].wrapping_add(op.phase_inc[k]);
        }
        op.fb_prev2 = op.fb_prev1;
        op.fb_prev1 = new_outs[i];
    }
    new_outs
}
```

Things to understand here:

- **Fixed 8 lanes regardless of density.** `stack_density` (1..8) selects how
  many lanes are *audible*; unused lanes are silenced by a **zero pan gain**, not
  a branch or a `continue`. They still tick — the loop stays vectorisable.
- **Contiguous EG-level mirror.** Each lane's EG is a fat struct; striding over
  8 of them per sample is cache-hostile. So the control-rate tick copies all
  levels into a flat `op_eg_level[i][k]` array once per block
  (`refresh_eg_levels`), and the hot loop reads only that.
- **Per-lane everything.** EGs, pitch EGs, feedback scale and LFO2 are all
  per-lane, because the mod matrix can perturb each unison instance
  independently (e.g. `voice-spread → eg-rate` makes lanes decay at slightly
  different speeds — the thing that makes a supersaw feel alive instead of like
  identical twins).
- **Determinism.** Per-lane spread/rand and LFO2 seeds are drawn from an
  xorshift seeded by `(note, velocity, alloc-counter)`, so offline renders are
  reproducible.
- **Nyquist fade.** Carriers whose frequency approaches Nyquist are faded out
  with a per-lane smoothstep (window `f/fs ∈ [0.45, 0.49]`), computed at block
  rate and applied as a plain multiply — no per-sample branch. Modulators are
  left at 1.0 (fading them would thin the FM index).

The scalar `Voice` mirrors all of this at width 1 and is the reference the
stack is tested against.

### 2.5 Envelopes ([`eg.rs`](crates/vxn2-dsp/src/eg.rs), [`envelope.rs`](crates/vxn2-dsp/src/envelope.rs))

Two families:

- **Per-op EG** (`eg.rs`) — the DX7 4-rate/4-level amplitude envelope. It ticks
  at control rate. Downward segments march in **log2 (linear-in-dB)**, giving
  the characteristic DX7 exponential taper; attack marches in linear amplitude
  (DX7-punchy). A `Lin` curve is a per-op escape hatch for patches that relied
  on the old square curve. See ADR 0007 for the level→amp formula (§6).
- **Patch-wide envelopes** (`envelope.rs`) — a **signed Pitch EG** (4-rate/4-level,
  −99..+99, output in semitones) and a **Mod Env** (ADSR with Lin/Exp shape).
  Both are general matrix sources; the Pitch EG additionally sums into pitch by
  default.

All of these expose `scale_rates()` so a matrix `eg-rate` route can multiply a
cooked envelope's speed per lane.

### 2.6 The optional filter ([`filter.rs`](crates/vxn2-dsp/src/filter.rs))

An **OTA-C ladder** (Roland/Juno flavour, not Moog): per-stage `tanh` saturation
rather than one global throat. Selectable LP/HP/BP/Notch × 2/4-pole,
self-oscillating at `k≈4`. Coefficients are frozen per block. It is **optional
and per-voice**, oversampled 1×/2×/4×/8× (default 4×) — but only the filter
kernel is oversampled; the FM operators stay at base rate. A single **shared
decimator** handles all voices because linear ops commute
(`decimate(Σ voices) ≡ Σ decimate(voice)`). See ADR 0004 (§6).

### 2.7 Effects ([`delay.rs`](crates/vxn2-dsp/src/delay.rs), [`reverb.rs`](crates/vxn2-dsp/src/reverb.rs), [`phaser.rs`](crates/vxn2-dsp/src/phaser.rs), [`dynamics.rs`](crates/vxn2-dsp/src/dynamics.rs))

"Clean, no character" — the patch's character lives in the FM, the FX add space:

- **Delay** — stereo, BPM-syncable, ping-pong, Catmull-Rom fractional read
  (no pitch-click when the delay time glides), DC blocker on the feedback path.
- **Reverb** — an 8-line **Feedback Delay Network** with a multiply-free 8×8
  Hadamard mixing matrix, mutually-prime delay lengths, per-line one-pole
  damping and a ±2-sample per-line LFO to suppress flutter.

```rust
// reverb.rs — multiply-free unitary mixing (fast Walsh–Hadamard)
fn hadamard8(mut x: [f32; LINES]) -> [f32; LINES] {
    for step in [4_usize, 2, 1] {
        let mut i = 0;
        while i < LINES {
            for j in i..i + step {
                let (a, b) = (x[j], x[j + step]);
                x[j] = a + b; x[j + step] = a - b;
            }
            i += step * 2;
        }
    }
    for v in x.iter_mut() { *v *= INV_SQRT8; }   // unitary after 1/√8
    x
}
```

The full serial chain (defined in the engine) is:
`cleanup(DC block) → dynamics → phaser → delay → reverb → master gain → limiter`.
Each block is bit-exact passthrough when its `*_on` toggle is off.

---

## 3. The engine (`vxn2-engine`)

This is where notes become sound. The `Engine` owns the voice allocator, the
mod matrix, the LFO1 state, all the FX blocks, per-voice filters, and a pile of
per-stack ramp/smoother scratch. It is driven one **control block** at a time
(32 samples — see §4). Everything is pre-allocated; the audio thread never
touches the heap and never panics across the FFI boundary.

### 3.1 Voice allocation ([`alloc.rs`](crates/vxn2-engine/src/alloc.rs))

`PolyAlloc` runs **20 physical stacks but caps active voices at 16**
(`N_ACTIVE = 16`, `N_DECLICK = 4`, `N_STACKS = 20`). The four spare stacks carry
declick tails without counting against polyphony — this is the mechanism that
makes voice stealing click-free (ADR 0008).

Each stack has an explicit lifecycle: `VoicePhase ∈ {Idle, Held, Releasing,
Declick}`. The core insight (ADR 0006/0008): a **fresh onset from silence is
already click-free** (the EG ramps up from ~0), whereas re-cooking a *sounding*
voice in place steps the FM spectrum audibly. So the main steal path is:

1. new note takes a spare **idle** stack and onsets fresh;
2. the victim is **declick-faded in place** (a forced ~5 ms EG release, all ops
   scaled to reach 0 together), keeping its slot and filter state so its tail
   rings continuously;
3. only a pathological steal-storm that exhausts all spares falls back to
   in-place reuse.

Victim selection prefers the **quietest** voice, keying up (pedal-held or
already `Releasing`) before actively-held keys, oldest as tiebreaker:

```rust
// alloc.rs — quietest-first, key-up-preferred voice stealing
fn pick_victim(&self) -> Option<usize> {
    let mut best: Option<(usize, f32, u64)> = None;
    let mut best_keyup: Option<(usize, f32, u64)> = None;
    let quieter = |c: (usize,f32,u64), b: Option<(usize,f32,u64)>| match b {
        Some(b) if b.1 < c.1 || (b.1 == c.1 && b.2 <= c.2) => b,
        _ => c,
    };
    for i in 0..N_STACKS {
        if matches!(self.stacks[i].meta.phase, VoicePhase::Held | VoicePhase::Releasing) {
            let cand = (i, self.stacks[i].carrier_level(), self.seq[i]);
            best = Some(quieter(cand, best));
            if self.held_by_pedal[i] || self.stacks[i].meta.phase == VoicePhase::Releasing {
                best_keyup = Some(quieter(cand, best_keyup));
            }
        }
    }
    best_keyup.or(best).map(|(i, _, _)| i)
}
```

The allocator also owns **Poly/Solo** modes, sustain-pedal deferral, and
**glide (portamento)** — which is *always-glide* (VXN1 parity): every note after
the first slides from the previous sounding pitch, tracked via a block-rate
`GlideState`.

### 3.2 The mod matrix ([`matrix.rs`](crates/vxn2-engine/src/matrix.rs), [`modulation.rs`](crates/vxn2-engine/src/modulation.rs))

The matrix is **the only routing mechanism** — there is no hard-wired "mod
wheel → cutoff". 16 slots, each `(source, dest, depth, curve)`. Depths for
slots 1–8 are CLAP-automatable; slots 9–16 and all topology (source/dest/curve)
are patch state only.

Sources and destinations live in **three granularity tiers**:

| Tier | Sources | Destinations |
|------|---------|--------------|
| **patch-global** (1/patch) | `lfo1`, `mod-wheel`, `aftertouch` | `lfo1-rate`, `delay-mix`, `reverb-mix` |
| **per-stack** (1/voice) | `pitch-eg`, `mod-env`, `velocity`, `key` | `lfo2-rate`, `stack-detune`, `stack-spread`, `cutoff`, `resonance`, filter-drive |
| **per-lane** (1/unison lane) | `lfo2`, `voice-idx`, `voice-spread`, `voice-rand` | `op{1..6}-{pitch,level,pan}`, `global-pitch`, `feedback`, `lfo2-phase`, per-op stack-pitch |

A routing is **coherent** iff `source tier ≤ dest tier`. A coarser source
broadcasts cleanly to a finer dest; a *finer* source into a coarser dest
collapses to lane 0 (lossy) and the UI renders it red. `coherence(src, dst)` is
the single source of truth, consulted by the UI tooltip, the matrix eval, and a
**factory CI test** that fails if any shipped preset routes incoherently.

Evaluation is a two-phase, allocation-free fan-out done once per block:

- `eval_sources(...)` broadcasts every source into a `[lane][source]` lookup
  table (the broadcast cost is paid once, never inside the per-slot loop);
- `eval_dests(...)` walks each active slot, applies its curve, multiplies by
  depth, and accumulates into the matching per-lane destination column.

**Rate control:** non-pitch destinations are applied at block rate. Pitch
destinations are smoothed down to a 16-sample sub-block quantum (`PitchSmoother`)
so a per-block matrix value doesn't zipper the pitch — and once the smoother is
within `1e-4` semitones of target the per-quantum re-cook is skipped entirely, so
unmodulated pitch pays nothing.

**Stack-pitch modulation** (ADR 0005): six `Op{N}StackPitch` destinations. A
single route bends the target op *and every op in its connected component of the
algorithm graph* by the **same semitone delta**, keeping FM ratios intact
(frequencies scale by `2^(Δst/12)`, ratios invariant). Fixed-Hz ops are graph
walls. The component mask is cached per cook, re-solved only on algo change or a
Ratio↔Fixed flip.

### 3.3 The render loop ([`engine.rs`](crates/vxn2-engine/src/engine.rs))

`process_block(out_l, out_r)` is the entry point. The order is strict and
matters — reordering silently introduces one-block-latency bugs, which is why
[`tests/baseline.rs`](crates/vxn2-engine/tests/baseline.rs) hashes the rendered
output as a guard:

1. Allocator block-tick (advance glide ramps).
2. Tick LFO1 (block rate, BPM-synced) and every stack's EGs (control rate).
3. **`cook_stacks_block`** — a strict 12-stage per-stack loop (idle-skip →
   fresh-note detect → LFO2 → source fan-out → matrix eval + stack-pitch scatter
   → target projection → pitch smoother → level+EG rebase → ramp compute →
   feedback+detune → deferred LFO2 phase/rate → spread/FX aggregation).
4. Dispatch to a render body: `render_block_off` (unfiltered, sample-major),
   `render_block_filtered` (stack-major oversampled), or
   `render_block_filter_xfade` (an ~8 ms raised-cosine blend when the filter is
   toggled, so enabling it doesn't click).
5. FX chain, master gain, optional limiter.

Two subtleties encoded in that loop:

- **Fresh-note detection** compares the allocator's per-slot generation counter
  against a mirror; a bump means the slot was reused, so macro-mod smoothers are
  *zeroed* (snap, not glide from a stale previous voice).
- **Level modulation is multiplicative on the EG**: effective level =
  `clamp(eg · (1 + m), 0, 1)`, with a block-edge rebase so the per-sample ramp
  stays continuous across an EG step. Because it multiplies the EG, a released op
  (`eg → 0`) always closes — no drone-through-zero.

### 3.4 Shared state and the param table ([`shared.rs`](crates/vxn2-engine/src/shared.rs), [`params.rs`](crates/vxn2-engine/src/params.rs))

`SharedParams` is the **lock-free SPSC bridge** between the UI/host and the
engine: a flat `[AtomicU32]` of f32 bits, plus packed atomics for matrix rows
and KS/EG curve metadata, plus the **dirty bitset** (§5). Reads use `Relaxed`;
the write→dirty-bit pairing uses `Release`/`Acquire` so the reader sees the
value before the bit.

`params.rs` is the CLAP parameter table. The authoritative count is:

```rust
// params.rs
pub const N_OPS: usize        = 6;
pub const N_PER_OP: usize     = 22;                             // repeated per op
pub const N_PER_PATCH: usize  = N_OPS * N_PER_OP + REST;        // 169
pub const N_PATCH_LEVEL: usize = 40;
pub const TOTAL_PARAMS: usize = N_PER_PATCH + N_PATCH_LEVEL;    // 209
```

> **Heads-up on the numbers:** the ADRs and `PARAMETERS.md` quote 180 / 186 /
> etc. — those are *historical* snapshots from before the filter, dynamics,
> phaser and per-op phase params landed. **`TOTAL_PARAMS = 209` in `params.rs`
> is the live source of truth**; treat the ADR figures as period records, not
> current counts.

The 22 per-op params repeat 6× with an `opN_` prefix (ratio mode / num / denom /
fine / detune / fixed-hz / level / vel-sens / eg r1–4 / eg l1–4 / ks break /
ks l-depth / ks r-depth / ks rate / pan / phase). Globals are named directly.
Param IDs are legible kebab-case strings but are **not** a stability constraint
(the patch format is name-keyed) — you may rename freely.

A neat safety net: every section boundary is pinned at compile time, so an
accidental mid-section param insert **fails to compile** instead of silently
shifting every downstream ID:

```rust
// shared.rs — compile-time layout guard
const _: () = assert!(id_eq(PARAMS[OFF_ALGO].id, "algo"));
const _: () = assert!(id_eq(PARAMS[OFF_FEEDBACK].id, "feedback"));
// ...one per section anchor
```

### 3.5 Presets ([`preset.rs`](crates/vxn2-engine/src/preset.rs), [`preset_io.rs`](crates/vxn2-engine/src/preset_io.rs), [`factory.rs`](crates/vxn2-engine/src/factory.rs), [`default_patch.rs`](crates/vxn2-engine/src/default_patch.rs))

Patches are **name-keyed, sparse TOML** — only non-default params are written,
so files are compact and diffable:

```toml
schema = 1
[meta]
name = "E.Piano 1"
category = "Keys"
[params]
op1-detune = 2.0
op1-level = 99
[[matrix]]
slot = 0
source = "lfo2"
dest = "global-pitch"
depth = 0.0
```

The **factory bank** is embedded into the binary at compile time with
`include_dir!`. Note the gotcha (from project memory): editing or adding a
factory TOML does *not* recompile it, because `include_dir!` emits no
`rerun-if-changed` — you must touch `factory.rs` (or run the xtask install)
before it re-bundles. `default_patch.rs` is the DX7 ROM1A "E.PIANO 1"
transcription and is the deterministic initial state. `preset_io.rs` handles
per-OS user-preset directories with path-canonicalisation guards so a browser
op can't escape the user dir. The host state blob is a separate versioned binary
format (`b"VXN2"` magic) used for DAW session save/load.

---

## 4. The CLAP shell (`vxn2-clap`)

[`lib.rs`](crates/vxn2-clap/src/lib.rs) wires the plugin to the host via
`clack`. It declares the params / state / audio-ports / note-ports / gui /
timer extensions and runs the process callback. The two threads are strictly
separated: **audio thread** (process, event dispatch) holds no locks;
**main thread** (GUI timer, param flush) owns the controller.

The process callback chunks the host buffer into fixed **32-sample control
blocks** so LFO/matrix/EG state advances at a predictable rate regardless of the
host's buffer size, and dispatches host events at their sample boundaries:

```rust
// vxn2-clap/src/lib.rs — process callback (abridged)
let _ftz = ScopedFlushToZero::new();
self.local.fetch_ui_changes(&self.shared.params);   // pull UI/preset writes
self.local.write_to(self.engine.params_mut());
self.engine.apply_block_params();
// transport: stop → all_notes_off (stuck-note guard); HAS_TEMPO → set_tempo
for event_batch in events.input.batch() {
    for event in event_batch.events() {
        dispatch_event(engine, local, shared, event);   // ParamValue vs note/MIDI
    }
    let (start, end) = batch_range(event_batch.sample_bounds(), frames);
    for (a, b) in control_chunks(start, end) {           // 32-sample slices
        engine.process_block(&mut l[a..b], &mut r[a..b]);
    }
}
self.local.emit(&self.shared.params, events.output, frames as u32);   // echo UI edits
```

Host param events are folded into a thread-local `LocalParams` mirror
([`local.rs`](crates/vxn2-clap/src/local.rs)) *and* written through to
`SharedParams` (atomic store + dirty bit), with the value read back clamped to
keep the mirror in lockstep — no mutex on the audio path. `emit` sends
UI-originated edits back to the host wrapped in CLAP gesture brackets;
host-originated automation is deliberately **not** echoed (tracked via a
`ui_changed` flag that only UI writes set).

[`gui.rs`](crates/vxn2-clap/src/gui.rs) implements the CLAP GUI extension:
`set_parent` spawns the `vxn2-ui-web` webview as a child of the host window and
registers a ~16 ms main-thread timer; `destroy` tears it down. Timer
registration is best-effort — if the host lacks timer support the editor is
static but gestures still post.

---

## 5. UI plumbing — the dirty-bitset pump (`vxn2-app`, ADR 0003)

This is the single most important architectural idea on the UI side, and worth
internalising.

**The problem:** UI edits, host automation and preset loads all flow through the
same `SharedParams` store. If the controller echoed UI writes *and* a pump also
re-broadcast them, the page would get duplicates.

**The solution — single-emitter discipline:** every write flips a bit in an
atomic dirty bitset, and the main-thread timer drain is the **only** Model→View
channel. The controller's echo is explicitly disabled:

```rust
// vxn2-clap/src/lib.rs
controller.set_echo_param_writes(false);   // pump is the sole emitter
```

Each timer tick: pull UI intents into the model (`tick_vxn2`), drain the
controller's view queue, then **drain the dirty bits** and flush everything to
the webview in a single `evaluate_script` call:

```rust
// vxn2-clap/src/lib.rs — drain (abridged): coalesced per-id + snapshots
let value_bits = params.take_dirty_values();
for (w, mut bits) in value_bits.iter().copied().enumerate() {
    while bits != 0 {
        let id = w * 64 + bits.trailing_zeros() as usize;
        bits &= bits - 1;                                  // pop lowest set bit
        out.push(ViewEvent::ParamChanged {
            id: ParamId::new(id),
            plain: params.get(id),
            norm:  params.get_normalised(id),
            display: sync_aware_display(params, id, params.get(id)),
        });
        // if this id is a sync flag, queue its rate partner for a display refresh
    }
}
if params.take_dirty_matrix() != 0 { out.push(matrix_snapshot_event(params)); }
```

Properties that fall out of this: N writes to one param between ticks **coalesce**
to one event; the whole 16-row matrix ships as one `MatrixSnapshot` rather than
16 events; and on a fresh `SharedParams` all bits are set, so the first drain
broadcasts the entire table (the page's initial seed).

`vxn2-app` ([`controller.rs`](crates/vxn2-app/src/controller.rs),
[`events.rs`](crates/vxn2-app/src/events.rs), [`model.rs`](crates/vxn2-app/src/model.rs))
holds the MVC glue: the **view never reads the model**, a dumb pump observes the
model and emits events, and the controller translates VXN2-specific custom
intents (`Vxn2UiCustom`: op-tab, matrix row, KS/EG curve, rebroadcast) into
model writes. Matrix topology and KS/EG curves are the non-CLAP state that rides
these custom events rather than the param table.

---

## 6. The web faceplate (`vxn2-ui-web`)

The UI is a **fully embedded static HTML/JS/CSS bundle** — every asset is pulled
in with `include_str!` at compile time; there is no runtime filesystem
dependency (a `VXN2_DEV_ASSETS=1` dev mode reads from disk for hot iteration).
At build time the bootstrap splices in Rust-generated JSON: the param
descriptor table (`build_params_json`), the matrix source/dest/curve lists +
the **coherence matrix** (`build_matrix_lists_json`), the default patch and the
tempo subdivisions.

The message protocol is **JSON over the wry WebView bridge**
(`window.ipc.postMessage`). Standard CLAP-shaped opcodes (`set_param`,
`begin_gesture`/`end_gesture`, `request_text_input`) come from the shared
`vxn-core-ui-web` layer; VXN2 adds custom opcodes for the state that isn't in
the param table:

| Direction | Opcodes |
|-----------|---------|
| UI → engine | `set_op_tab`, `set_matrix_row`, `request_matrix_snapshot`, `set_ks_curve`, `request_ks_curve_snapshot`, `set_eg_curve`, `request_eg_curve_snapshot`, `request_full_rebroadcast` |
| engine → UI | `param_changed`, `matrix_snapshot`, `ks_curve_snapshot`, `eg_curve_snapshot`, `op_tab_changed` |

The faceplate is laid out as three rows (op row: algorithm diagram + op tabs +
per-op detail; global mod row: LFO1/LFO2/Pitch EG/Mod Env; performance row:
voice/stack/filter/FX/master), with the mod matrix in a button-triggered
overlay. Widgets are vanilla-JS primitives (no framework). A couple of choices
worth flagging:

- **Taper math is mirrored exactly** between the JS faders and the Rust param
  descriptors, so a knob feels identical whether dragged in the UI or moved by
  host automation.
- **UI edits paint optimistically** — a drag updates the DOM immediately and the
  engine's `param_changed` echo (1 tick later) reconciles; the latency is
  imperceptible.
- **JS/engine table drift is caught by tests.** For example a Rust test parses
  the hardcoded JS `ALGO_CARRIERS` table and asserts it matches
  `vxn2_dsp::algo::ALGOS` — so the op-tab carrier colouring can't silently
  diverge from the engine.

Faceplate assets are pure DOM (no Web Audio / AudioWorklet), so the Safari
audio-glitch issues noted for VXN1's web build don't apply here.

---

## 7. ADR quick-reference

The design decisions, with their load-bearing numbers:

- **[0001](adrs/0001-vxn2-overall-design.md) — Overall design.** 6 ops, 32 DX7
  algorithms, one feedback path per algorithm, stacking as first-class,
  Bhaskara sine + Q32 phase, matrix-as-only-routing, clean delay+FDN reverb.
- **[0002](adrs/0002-drop-dual-layer.md) — Drop dual-layer.** Whole/Layer/Split
  is gone; one patch = one parameter set. Layer's job is covered by algorithm +
  matrix combinatorics; keyboard splits are the DAW's job. Halved the param
  surface and removed a `PolyAlloc` layer dimension.
- **[0003](adrs/0003-dirty-bitset-diff-pump.md) — Dirty-bitset pump.** Single
  atomic bitset is the only Model→View bridge; `fetch_or(Release)` on write
  pairs with `swap(Acquire)` on the sole reader. Coalesces, dedups, batches
  matrix snapshots. (§5.)
- **[0004](adrs/0004-optional-per-voice-oversampled-filter.md) — Optional
  oversampled filter.** OTA-C ladder, per-voice, 1×/2×/4×/8× (default 4×), only
  the kernel oversampled, one shared decimator. Toggle crossfades over ~8 ms
  (`0.5 − 0.5·cos(πt)`). Reports **no** latency (glitch-free OS switching beats
  PDC truth, since CLAP only allows latency changes at activate boundaries).
- **[0005](adrs/0005-stack-pitch-mod.md) — Stack pitch mod.** Six
  `OpNStackPitch` dests; a route bends a whole ratio-coherent FM component by
  one shared `Δst`, undirected propagation, fixed-Hz ops as walls, mask cached
  per cook.
- **[0006](adrs/0006-voice-lifecycle-click-free-reuse.md) — Voice lifecycle.**
  Explicit `VoicePhase`; onset-from-silence is click-free; declick = forced
  ~5 ms EG release (5 ms clears the ~0.06 transient of a single 0.67 ms block
  down to ~0.006); one extra idle-grace block before retire.
- **[0007](adrs/0007-dx7-log-level-curve.md) — DX7 log level curve.** One shared
  curve for op output level and EG L-values: `level_to_amp(L) = 2^((L−99)/8)`
  for L∈[1,99], `0` at L=0. ≈0.75 dB/step (−37 dB at L=50, −74 dB at L=1).
  Evaluated only in `cook`.
- **[0008](adrs/0008-declick-headroom-voice-stealing.md) — Declick-headroom
  stealing.** 20 physical stacks, 16 active cap, 4 declick spares; poly steal
  takes a spare and onsets fresh while the victim declicks in place — same
  proven click-free path as solo. (§3.1.)

---

## 8. Constraints you must respect when editing

- **Audio thread: no allocation, no panic, no locks.** All buffers are made in
  `Engine::new`. Corrupt IDs from the host decode to safe sentinels, never
  `unwrap`. `SharedParams` is `Relaxed` SPSC.
- **The hot loops must stay branch-free and vectorisable.** No runtime `match`
  inside the per-lane op loop (it silently kills NEON — see the VXN1 lessons in
  project memory). Silence lanes with zero gain, not `continue`. Verify codegen
  with an `objdump`, and beware the ARM64 grep pitfall (the `.4s` suffix is on
  the mnemonic, not the operands).
- **Control-rate vs sample-rate discipline.** Envelopes, LFOs and matrix sources
  tick once per 32-sample block; only pitch is sub-block-smoothed (16-sample
  quantum). Don't move block-rate work into the sample loop.
- **Render-order is load-bearing.** The 12-stage `cook_stacks_block` order and
  the `process_block` pipeline order encode one-block-latency contracts;
  `tests/baseline.rs` (render hash) is the guard — expect it to fail loudly if
  you reorder.
- **Param-table edits.** Adding a param shifts IDs; the compile-time section
  guards in `shared.rs` will stop you if you insert mid-section. Prefer
  appending within the right section, re-run the layout asserts, and remember
  factory presets are name-keyed (safe) but the host state blob is versioned
  (bump it).
- **Factory presets don't auto-rebuild** — touch `factory.rs` after editing a
  TOML (`include_dir!` emits no `rerun-if-changed`).
- **Don't `git add -A` in this monorepo** — concurrent vxn-2 editor work
  pollutes commits; stage explicit paths (project memory).

---

*This manual is a map, not the territory — when a detail matters, read the
module and the cited ADR. Every code snippet above is abridged for clarity;
the real signatures live in the source.*
