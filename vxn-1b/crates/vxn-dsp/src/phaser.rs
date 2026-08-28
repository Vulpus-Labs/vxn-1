//! Re-export of `vxn_core_dsp::phaser`.
//!
//! Ticket 0228 (epic E041) unified this kernel with vxn-2's, which was the
//! superset: same allpass core and the same collapsed macro surface, plus a
//! params snapshot and an internal `WetFade` bypass. vxn-1b adopts the
//! superset, which changes its phaser in two known ways:
//!
//! - **The per-stage scatter is redrawn.** The ±3 % break-frequency spread is
//!   seeded from a PRNG, and the two crates used different generators —
//!   `vxn-dsp`'s plain `xorshift64` here, xorshift64\* there. Same distribution,
//!   different draw, so the four notches sit at slightly different offsets.
//! - **Bypass moved inside.** The 10 ms linear crossfade `FxChain` wrapped this
//!   slot in is gone; the kernel's own 30 ms `WetFade` owns the on/off glide,
//!   and `FxChain` true-skips on `is_active`. The mix knob is now smoothed too,
//!   where it used to step per control block.
//!
//! Steady-state audio with the phaser on differs only by the first of those.
pub use vxn_core_dsp::fx::FxKernel;
pub use vxn_core_dsp::phaser::{PhaserParams, StereoPhaser};
