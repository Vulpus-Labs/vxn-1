---
id: "0121"
title: Rust hygiene batch — transmute, factory cache, FTZ arm
priority: low
created: 2026-06-10
epic: E021
---

## Summary

Batch of small Rust items from the 2026-06-10 review, none
worth a ticket alone. Two have teeth (the `from_index`
transmute footgun, the silent FTZ no-op on unsupported
targets); the rest is polish. One commit per logical item or
one batch commit — author's call; behaviour must not change
except where noted.

## Acceptance criteria

vxn-app:

- [ ] `PatchParam::from_index` / `GlobalParam::from_index`
      (`params.rs:219, 291`): replace `unsafe transmute`
      with a macro-generated exhaustive `match` (or
      equivalent safe construction). Existing round-trip
      tests still pass; no `unsafe` remains in params.rs.
- [ ] `PatchParam::CutoffTuned` descriptor/variant gains a
      comment: UI-only display-mode toggle, engine
      deliberately never reads it, persisted as a param so
      it travels with presets/state (review initially
      misread it as dead — prevent the rerun).

vxn-engine:

- [ ] `factory()` (`factory.rs:68`) caches parsed bank in
      `std::sync::OnceLock` — currently re-parses all ~33
      TOML files per call, 3× per browser interaction.
      Main-thread only; no RT implication.
- [ ] `rename_user_preset` (`preset_io.rs:515`): unknown-key
      warnings from the round-trip parse logged (or
      propagated) instead of silently discarded via `_w`.

vxn-dsp:

- [ ] `enable_flush_to_zero` (`lib.rs:87-103`): explicit
      `#[cfg(not(any(x86_64, aarch64)))]` arm — at minimum a
      `compile_error!` or documented no-op comment, since
      phaser/BBD denormal behaviour depends on FTZ being
      armed.
- [ ] Broken rustdoc links `[crate::ladder]` /
      `[crate::ladder::LadderKernel]` in `ota_ladder.rs`
      fixed (module never shipped).
- [ ] Cross-reference comments between the duplicated Padé
      tanh coefficient sets (`math.rs:43-46` ↔
      `poly.rs:53-58`) so an edit to one finds the other.
      Do NOT merge the implementations — branch/branchless
      split is deliberate (memory: vxn1-tanh-branchless-
      only).
- [ ] `HALF_SEMITONE_VOCT` (`random_walk.rs:15`): demote to
      `pub(crate)` or remove (exported, never consumed).
- [ ] `flush_denormal` NaN-zeroing behaviour reflected in
      the doc comment header, not just the inline note (in
      vxn-core-utils; name stays).
- [ ] New test: `AdsrShape::Exponential` attack overshoot
      cap at 1.0 and release snap-to-idle at `EXP_SNAP_EPS`;
      one-line doc comments on `EXP_N_TAU` /
      `EXP_ATTACK_TARGET` (`adsr.rs:23-24`).

xtask:

- [ ] `LIB_NAME` derived from the manifest (env var
      `CARGO_PKG_NAME` of vxn-clap or parse) instead of the
      hardcoded `"vxn_clap"` constant — or a comment
      accepting the coupling. Crate rename must not silently
      produce an empty bundle.

Global:

- [ ] `cargo test --workspace` green; `tests/baseline.rs`
      hash unchanged (nothing here touches the render path).

## Notes

The transmute is sound today (`i < Self::COUNT` guard,
contiguous `#[repr(usize)]` discriminants) — the risk is
the *next* enum edit, and the param enum is edited every
feature. That is why it leads the batch.

Per-crate asm checks are misleading pre-LTO (memory:
vxn1-ota-filter-perf) — no perf verification needed here;
nothing touches a hot path.
