---
id: "0019"
product: vxn-1
title: Rust hygiene batch — transmute, factory cache, FTZ arm
priority: low
created: 2026-06-10
epic: E011
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

## Close-out (2026-06-22)

Behaviour-preserving batch. `cargo test --workspace` green for every
touched crate; `tests/baseline.rs` hash unchanged (nothing here touches
the render path).

**vxn-app:**

- `PatchParam`/`GlobalParam` `from_index`: the two `unsafe transmute`s are
  gone. A new `indexed_param_enum!` macro defines each enum plus a safe
  exhaustive-`match` `from_index` (and `COUNT`/`index`/`all`) from a single
  variant list, so a future variant can't desync `from_index`. No `unsafe`
  remains in `params.rs`; `from_index_roundtrips` still passes.
- `cutoff_tuned` descriptor gained the comment: UI-only display-mode
  toggle, engine deliberately never reads it, persisted so it travels with
  presets/state (prevents the next reviewer re-flagging it as dead).

**vxn-engine:**

- `factory()` now returns `&'static [FactoryPreset]` cached in a
  `OnceLock` — parsed once instead of re-parsing ~33 TOML files per call
  (3× per browser interaction). Callers are all read-only (`len`/`get`/
  `iter`). Main-thread only.
- `rename_user_preset`: round-trip parse warnings are logged to stderr
  (host-captured) instead of dropped via `_w` — a rename rewrites the file,
  so an unknown-key warning means possible data loss.

**vxn-dsp:**

- `enable_flush_to_zero`: explicit `#[cfg(not(any(x86_64, aarch64)))]` arm
  (documented no-op) so a new target compiles with a greppable
  "denormals unhandled here" rather than silently.
- `ota_ladder.rs`: the four broken `[crate::ladder]` /
  `[crate::ladder::*]` rustdoc links (module never shipped) de-linked to
  prose.
- `math::fast_tanh` ↔ `poly::oscillator::tanh_c`: cross-reference comments
  on the shared Padé(5,6) coefficients; NOT merged (branched vs branchless
  split is deliberate — memory `vxn1-tanh-branchless-only`).
- `HALF_SEMITONE_VOCT` removed (was `pub`, consumed nowhere); a breadcrumb
  comment points at git history.
- `flush_denormal` doc header now states it also zeroes NaN/±∞ (not just
  the inline note).
- New `adsr` test `exponential_attack_caps_at_one_and_release_snaps_to_idle`
  (overshoot cap at 1.0, release snap-to-idle); one-line doc comments on
  `EXP_ATTACK_TARGET` / `EXP_N_TAU` / `EXP_SNAP_EPS`.

**vxn-core-utils:** `flush_denormal` lives here — doc updated as above.

**xtask:** `LIB_NAME` keeps the hardcoded `"vxn_clap"` with a comment
accepting the coupling to `--package vxn-clap`; `build_universal` already
errors with the path if the renamed dylib is absent, so a rename can't
silently ship an empty bundle (the ticket's accepted alternative).
