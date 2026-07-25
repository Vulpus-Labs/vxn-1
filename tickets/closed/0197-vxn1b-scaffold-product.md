---
id: "0197"
product: vxn-1b
title: "Scaffold vxn-1b product: crates, workspace wiring, shared vxn-dsp dep, stub CLAP that loads"
priority: high
created: 2026-07-25
epic: E036
---

## Summary

Stand up the VXN1b product tree as a sibling to `vxn-1/`, `vxn-2/`, `vxn-3/`.
Create the three forked crates and an xtask, wire them into the root Cargo
workspace, and take a **direct dependency on VXN1's `vxn-dsp`** (no DSP fork —
[ADR 0001](../../vxn-1b/adrs/0001-vxn1b-overall-design.md) §1). The deliverable
is a stub CLAP that loads in a host; no synthesis or matrix yet.

Crates:

- `vxn-1b/crates/vxn1b-engine` — will hold the param table + matrix evaluator
  (later tickets). For now: depends on `vxn-dsp`, empty/skeleton lib.
- `vxn-1b/crates/vxn1b-clap` — clack cdylib, stable unique plugin id, minimal
  params/state (can be empty), loads in a host.
- `vxn-1b/crates/vxn1b-ui-web` — skeleton (faceplate lands in [[E038]]).
- `vxn-1b/xtask` — `bundle` command that builds/installs the `.clap` (fork the
  vxn-1 xtask; watch the two-`.parent()` workspace-root quirk from
  `vxn2-xtask-flat-workspace`).

## Acceptance criteria

- [ ] `vxn-1b/crates/{vxn1b-engine, vxn1b-clap, vxn1b-ui-web}` + `vxn-1b/xtask`
      exist and are listed in the root `Cargo.toml` `members`.
- [ ] `vxn1b-engine` depends on `vxn-1/crates/vxn-dsp` and the shared
      `vxn-core-*` crates; `cargo build -p vxn1b-clap` succeeds.
- [ ] `vxn-1b/xtask bundle` produces a `.clap` that loads in a DAW (params/state
      may be empty at this stage).
- [ ] The CLAP plugin id is unique and distinct from vxn-1/2/3.
- [ ] No `git add -A` — stage explicit paths (`vxn-concurrent-vxn2-work-no-git-add-all`).

## Notes

- Follow the vxn-1/vxn-2/vxn-3 crate-per-product pattern; copy the leanest of the
  three xtasks as the base.
- Root `Cargo.toml` members list is at `../../Cargo.toml`.
- Shared crates available: `vxn-core-utils`, `vxn-core-app`, `vxn-preset`,
  `vxn-core-ui-web`, `vxn-core-clap`.
- This is pure scaffolding — DSP reuse means later tickets add behaviour by
  wiring `vxn-dsp` kernels, not porting them.

## Close-out (2026-07-25)

- Product tree stood up as a sibling to vxn-1/2/3: `vxn-1b/crates/{vxn1b-engine,
  vxn1b-clap, vxn1b-ui-web}` + `vxn-1b/xtask`, all added to the root
  [Cargo.toml](../../Cargo.toml) `members` (and `vxn1b-engine`/`vxn1b-ui-web`
  path deps). No `vxn1b-app`/`vxn1b-dsp` crate — DSP is shared, app glue arrives
  with the GUI (0204).
- `vxn1b-engine` takes the direct `vxn-dsp` dependency (ADR 0001 §1) plus
  `vxn-core-utils`; the dep is genuinely linked, not just declared —
  `MAX_VOICES` re-derives from `vxn_dsp::MAX_VOICES`
  ([lib.rs:16](../../vxn-1b/crates/vxn1b-engine/src/lib.rs#L16)). Silent stub
  `Engine` (new/process_block/reset). Tests `vxn1b_engine::tests::{renders_silence_into_dirtied_buffers,
  inherits_voice_count_from_shared_dsp}` pass.
- `vxn1b-clap` is a loadable cdylib+rlib shell
  ([lib.rs](../../vxn-1b/crates/vxn1b-clap/src/lib.rs)): stereo out + note-in
  ports, silent render, no params/state/gui yet (→ 0200–0204). `cargo build -p
  vxn1b-clap` succeeds.
- Plugin id `labs.vulpus.vxn1b` — unique vs vxn1/2/3 (grep sweep over
  `vxn-1{,b} vxn-2 vxn-3` `.rs` confirmed the four distinct ids).
- `cargo run -p vxn1b-xtask -- bundle` produces `target/release/vxn1b.clap`
  (Contents/{MacOS/vxn1b, Info.plist, PkgInfo}); `_clap_entry` is exported.
  `clap-validator validate` → 0 failed, 10 passed, 11 skipped (params/state
  skips expected for a stub) — the plugin loads clean.
- Staged explicit paths only, no `git add -A` (per `vxn-concurrent-vxn2-work-no-git-add-all`).
</content>
