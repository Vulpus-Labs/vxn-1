//! Bench harness for VXN2. Wraps `vxn2-dsp` primitives in fixtures suitable
//! for criterion runs. The sine readers and the operator core live in
//! `vxn2-dsp`; this crate keeps a minimal scalar ADSR around as a stand-in
//! envelope for the multi-op algorithm benches that pre-date the full EG.

pub use vxn2_dsp::sine::{SINE_TABLE, TABLE_LEN, TABLE_MASK, scalar};

#[cfg(target_arch = "aarch64")]
pub use vxn2_dsp::sine::neon;

/// Minimal scalar ADSR fixture for op-level env benches. Linear stages.
/// Kept here because the multi-op benches predate the 4R/4L EG and use
/// a simpler envelope to isolate sine-throughput cost.
pub mod adsr {
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Stage {
        Idle,
        Attack,
        Decay,
        Sustain,
        Release,
    }

    #[derive(Clone, Copy)]
    pub struct Adsr {
        pub level: f32,
        pub stage: Stage,
        pub a_inc: f32,
        pub d_inc: f32,
        pub sustain: f32,
        pub r_inc: f32,
    }

    impl Adsr {
        pub fn new(a_inc: f32, d_inc: f32, sustain: f32, r_inc: f32) -> Self {
            Self {
                level: 0.0,
                stage: Stage::Idle,
                a_inc,
                d_inc,
                sustain,
                r_inc,
            }
        }

        #[inline]
        pub fn tick(&mut self, gate: bool, triggered: bool) -> f32 {
            if triggered {
                self.stage = Stage::Attack;
            }
            match self.stage {
                Stage::Idle => self.level = 0.0,
                Stage::Attack => {
                    self.level += self.a_inc;
                    if self.level >= 1.0 {
                        self.level = 1.0;
                        self.stage = Stage::Decay;
                    }
                }
                Stage::Decay => {
                    self.level -= self.d_inc;
                    if self.level <= self.sustain {
                        self.level = self.sustain;
                        self.stage = Stage::Sustain;
                    }
                }
                Stage::Sustain => {
                    if !gate {
                        self.stage = Stage::Release;
                    }
                }
                Stage::Release => {
                    self.level -= self.r_inc;
                    if self.level <= 0.0 {
                        self.level = 0.0;
                        self.stage = Stage::Idle;
                    }
                }
            }
            self.level
        }

        #[inline]
        pub fn is_sustain(&self) -> bool {
            matches!(self.stage, Stage::Sustain)
        }

        pub fn force_sustain(&mut self, level: f32) {
            self.stage = Stage::Sustain;
            self.level = level;
        }
    }
}
