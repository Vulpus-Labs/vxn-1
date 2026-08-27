---
id: E046
product: monorepo
title: "Uniform dirty-bitset Model→View pump for vxn-1 and vxn-1b"
status: open
created: 2026-08-25
---

> **vxn-1 retired, 2026-08-27.** The original vxn-1 is archived under
> `archive/vxn-1/`, out of the workspace and not expected to compile.
> **vxn-1b is now the canonical virtual-analogue synth**, and it carries what
> was vxn-1's DSP: `vxn-dsp` moved to `vxn-1b/crates/vxn-dsp` with its name
> intact. Where this epic says "vxn-1" as an *adopter* of shared code, read
> **vxn-1b** — the kernels are the same ones. Where it names vxn-1's shells,
> engine or web port, that work is gone.

> vxn-2 replaced its poll-and-diff Model→View bridge with a dirty-bitset pump in
> [ADR 0003](../../vxn-2/adrs/0003-dirty-bitset-diff-pump.md) (2026-06-10, epic
> [[E005]]). vxn-1 predates it; **VXN1b post-dates it by six weeks and inherited
> the older idiom by forking vxn-1's shell**, without the question ever being
> asked. This epic asks it, and — if the answer holds — brings all three synths
> onto one discipline.

## Why (and why it is not performance)

ADR 0003 is explicit that speed was never the argument:

> The codebase already trusts the polling diff for 180 params at audio rate.
> **Cost is not the problem; coordination + coverage are.**

It names two shipped failure modes it existed to kill. Both are present in
vxn-1b today, and one is present in vxn-1:

**1. Coverage gap.** `last_seen: Vec<f32>` tracks only the CLAP value table, so
any non-automatable shared field reaches the view through a bespoke push. ADR
0003 predicted the consequence: *"Every new non-CLAP field will reintroduce this
bug class until the discipline is uniform."*

- vxn-1b hit it twice and answered with two memo-diffs:
  [`push_matrix_echo`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L350) (0247) and
  [`push_key_echo`](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L379) (0221) — the
  exact workaround-per-field pattern the ADR abolished. It has since added LFO 2
  link, layer copy and the scope tap on top.
- vxn-1 answered the same problem a *third* way: the `on_model_loaded` hook
  republishes key mode / split point after a known load
  ([controller.rs:118](../../archive/vxn-1/crates/vxn-app/src/controller.rs#L118)) —
  correct for loads, silent for anything else that moves them.

**2. Echo duplication.** vxn-2 turned the controller echo off (ticket 0067)
because the pump made it double traffic. vxn-1 and vxn-1b both run echo **and**
a diff poll, double-emitting every param and relying on the WebView deduping by
id in `flush_view_events` — a property of the sink, written down only in a
comment ([vxn1b-clap:515-518](../../vxn-1b/crates/vxn1b-clap/src/lib.rs#L515-L518)).

**3. VXN1b already half-adopted the pattern and got it wrong.**
`key_dirty` ([shared.rs:65](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L65)) *is*
a dirty flag for a non-CLAP field — but it has **two consumers**: the audio
thread's `take_key_state()` engine re-sync and the view echo. So the view can't
use it ([the comment at shared.rs:298-302](../../vxn-1b/crates/vxn1b-engine/src/shared.rs#L298-L302)
says as much) and memo-diffs instead. vxn-2's bitsets are single-reader by
contract; VXN1b conflated the two directions. This is the sharpest single
argument that the discipline, not the mechanism, is what's missing.

**4. The web ports pay for it twice.** VXN1b's web controller
([0290](../../tickets/open/0290-vxn1b-web-controller-cdylib.md)) has to hand-add
an explicit `broadcast_all_params()` after each of `restore_state`,
`import_toml` and `copy_layer` — three model writes with no notify path — plus a
pack-time display recompute, because there are no bits to drain. All four
workarounds evaporate under this epic.

## Goal

One Model→View mechanism per synth, source-agnostic: **every write to the shared
model flips a bit; the main-thread tick is the sole reader.** When this closes:

- `vxn-1`, `vxn-1b` and `vxn-2` share one `DirtyBits` primitive out of
  `vxn-core-utils` rather than three copies.
- Both `vxn-1b` shells (CLAP + web controller) and both `vxn-1` shells drain
  bits; `echo_param_writes` is off for all three synths.
- Deleted: `vxn-app::diff` (`nan_diff` + `diff_params`), `push_param_diffs` ×2,
  `push_matrix_echo`, `push_key_echo`, their memo fields, `WebModel`'s
  `last_seen`, vxn-1's `on_model_loaded` key/split republish, and 0290's three
  explicit broadcasts.
- Adding a new non-CLAP shared field is: declare bit, set on write, read in
  pump. No new echo call site.

## Scope

**In:** `vxn-engine`, `vxn-clap`, `vxn-web-controller` (incl. `WebModel`),
`vxn-app`; `vxn1b-engine`, `vxn1b-clap`, `vxn1b-web-controller`;
`vxn-core-utils` (new shared primitive); one `vxn-core-app` change (below).

**Out:** vxn-2 behaviour — it already has the pump and only adopts the hoisted
type. vxn-3 (its own param path, [[vxn3-host-param-table]]). The plugin→host
direction (`ui_changed`, gesture brackets) — different consumer, ADR 0003's own
open question, still open. Any DSP change.

## The shared-crate change

Once vxn-1 and vxn-1b go echo-off, `load_preset`'s **unconditional**
`broadcast_all_params()` ([controller.rs:435](../../crates/vxn-core-app/src/controller.rs#L435))
starts double-emitting for them exactly as it already does for vxn-2 (finding 1
of [0298](../../tickets/open/0298-vxn2-web-controller-smells.md)). `StateLoaded`
was gated for this reason in 0067 and `load_preset` was missed. 0306 gates it,
and is a hard dependency of both shell rewires — not an optional cleanup.

## Planned tickets

Chain: **0299 → 0300 → 0301 → 0306 → 0302 → 0303** (vxn-1b).

The vxn-1 arm (**0299 → 0304 → 0306 → 0305**) is **closed won't-do** with the
2026-08-27 retirement — see [[0304]] / [[0305]]. `vxn-app::diff`, the poll this
epic set out to delete, went with the archive. What remains is the vxn-1b chain,
which was always the one with the live pain and the web port to prove it
against.

- [ ] **0299** *(monorepo)* — `DirtyBits` in `vxn-core-utils`: hoist vxn-2's
      bitset + drain out of `vxn2-engine`, adopt it there with **no behaviour
      change** (REBASELINE commit). One primitive, three consumers.
- [ ] **0300** *(vxn-1b)* — ADR: the pump for VXN1b. Ports vxn-2 ADR 0003's
      reasoning and settles the `key_dirty` two-reader split before any code
      moves.
- [ ] **0301** *(vxn-1b)* — `vxn1b-engine::SharedParams`: `dirty_values`
      (185 ids → 3 words), a matrix dirty word, and split `key_dirty` into an
      audio-consumer flag and a view-consumer bit.
- [ ] **0306** *(monorepo)* — `vxn-core-app`: gate `load_preset`'s broadcast on
      `echo_param_writes`, matching `StateLoaded`. Touches all three synths.
- [ ] **0302** *(vxn-1b)* — `vxn1b-clap` rewire: drain in `on_timer`, delete
      `push_param_diffs` / `push_matrix_echo` / `push_key_echo` + memos, echo off.
- [ ] **0303** *(vxn-1b)* — `vxn1b-web-controller` rewire: drain bits, delete
      0290's three explicit broadcasts and its pack-time display recompute.
      **Depends on [[0290]] shipping first.**
- [x] **0304** *(vxn-1)* — **won't-do, vxn-1 retired.**
- [x] **0305** *(vxn-1)* — **won't-do, vxn-1 retired.**

## Risks

- **Two shipped, released products.** vxn-1 and vxn-1b both ship
  ([[vxn-release-process]]); this edits an audio-thread struct in each. The
  bitset itself is additive (one `fetch_or(Release)` per write) — the risk is in
  the *deletions* on the read side. Land engine and shell in separate commits so
  a bisect can separate them.
- **vxn-1 has two model impls.** `SharedParams` and the web port's `WebModel`
  ([lib.rs:70](../../archive/vxn-1/crates/vxn-web-controller/src/lib.rs#L70)) both impl
  `ParamModel`; the pump has to exist in both or vxn-1's web build silently keeps
  the old path. This is why 0304 covers both in one ticket.
- **vxn-1's web readback pump is load-bearing today.** `nan_diff` →
  [diff.rs:85](../../archive/vxn-1/crates/vxn-app/src/diff.rs#L85) is where vxn-1's
  browser build gets `sync_aware_display` and the rate-partner refresh at all.
  Deleting it before the bits replace it swaps every synced rate label for raw
  Hz. 0305 must sequence those two edits, not do them in parallel.
- **Regression surface is "the editor looks right".** Most of what this touches
  has no audible signature, so `cargo test` green proves little.
  [[verify-audio-in-reaper]] applies: each shell rewire needs a manual DAW pass
  (load a preset, automate from the host, undo, reopen the editor) before close.
- **Nothing here is urgent.** Every symptom is already worked around and shipped.
  If the ADR in 0300 concludes the workarounds are cheaper than the churn, the
  right outcome is to close this epic unbuilt with that written down.

## Acceptance

- One `DirtyBits` primitive, three consumers, no per-synth copy.
- `echo_param_writes(false)` in all four shells (vxn-1 CLAP + web, vxn-1b CLAP +
  web), with a test per shell asserting one `ParamChanged` per model write.
- The deletion list under **Goal** is empty of survivors.
- A non-CLAP field changed by any writer (UI, host automation, state load, preset
  load, layer copy) reaches the view with no bespoke push — one test per synth
  proving it for its matrix / key state.
- Both products' full suites green; one manual DAW pass each,
  [[verify-audio-in-reaper]].
