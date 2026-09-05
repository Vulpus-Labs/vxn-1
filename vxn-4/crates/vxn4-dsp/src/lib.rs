//! VXN4 DSP kernels — the 8-operator phase-modulation block and the
//! band-limited wavetables it reads.
//!
//! This crate exists to be **measured before it is designed**. The vxn-4 brief
//! leaves three numbers open, and each of them moves the polyphony ceiling by
//! more than the rest of the architecture put together:
//!
//! - **Oversampling factor** — 8x or 16x at the operator block.
//! - **Table length** — the mip-0 length, which sets the working set per voice
//!   and therefore whether the gathers land in L1.
//! - **Lane-loop layout** — SIMD across *voices* ([`ops::VoiceMajor`]) or
//!   across *operators* ([`ops::OpMajor`]).
//!
//! Both layouts are here, they are built from the same configuration, and
//! `ops::tests::layouts_agree_bit_exactly` pins them to bit-identical output —
//! so the bench measures layout, not two different synths.
//!
//! ## What is deliberately not here
//!
//! The decimator. The brief's chain is 16x operators → 4x FX → 1x out, but the
//! decimator runs on the **stereo sum bus**, not per voice, so its cost does
//! not scale with polyphony and it cannot change the answer this bench exists
//! to give. [`ops::VoiceMajor::render`] closes the sum bus with a boxcar
//! average over each oversampled group, which is a placeholder for a polyphase
//! half-band cascade and is marked as one at the call site.
//!
//! Envelopes, the mod matrix and the FX block are likewise absent. Per-route
//! modulation is represented only by its *shape* — see [`ops::CompiledRouting`]
//! for what that costs and what it assumes.
//!
//! ## Audio-thread regime
//!
//! ADR 0002 §4 applies: plain `#[inline]` in sample loops, and **no `dyn` and
//! no enum-match inside a lane loop**. The lookup-strategy choice is the one
//! runtime decision this crate has, and it is dispatched through the
//! [`wavetable::Lookup`] marker types so it resolves at monomorphisation and
//! never reaches the loop body.
//!
//! Measure vectorisation with `llvm-objdump` on a linked bench binary, never
//! `cargo rustc --emit asm` on this library — `[profile.release]` sets thin LTO,
//! so cargo defers to link time and a trivially vectorisable loop shows up
//! scalar here. Two claims in E049's tickets were wrong before that was caught.

// The lane loops index by `0..V` and `0..NOPS` rather than iterating, which
// clippy flags. Left as indexing deliberately: the vector widths in
// `wavetable::WaveTable::lookup_unchecked`'s table were measured on this exact
// form, and ADR 0002 §4's whole point is that changing the idiom moves codegen.
// Rewriting them to iterators would silently invalidate every number in
// `vxn-4/README.md` without failing a test.
#![allow(clippy::needless_range_loop)]

pub mod ops;
pub mod wavetable;

pub use ops::{CompiledRouting, NOPS, OpConfig, OpMajor, Routing, VoiceMajor, note_to_freq};
pub use wavetable::{
    MIN_LEN, N_MIPS, Lookup, Plain, PlainUnchecked, Tap, ValueSlope, ValueSlopeUnchecked, WaveBank,
    WaveTable, Waveform,
};
