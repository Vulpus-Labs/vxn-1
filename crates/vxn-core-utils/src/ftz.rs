//! RAII guard that enables flush-to-zero on the current thread for the
//! lifetime of one audio block, restoring the previous FP control word on
//! drop.
//!
//! Denormal arithmetic costs ~100× and silently wrecks real-time deadlines
//! in filter/delay/reverb feedback paths. Hold one at the top of every
//! `process()` call.
//!
//! Per-process (not once in `activate`) because the FP control word is
//! thread-local and the host may dispatch `process` on a different thread.
//! Restoring on drop keeps us from perturbing the FP mode the host or
//! downstream plugins see after we return.
//!
//! ```ignore
//! let _ftz = ScopedFlushToZero::new();
//! // ... render the block; all SSE/NEON ops run flush-to-zero ...
//! ```

#[must_use = "FTZ is restored when the guard drops; bind it for the whole block"]
pub struct ScopedFlushToZero {
    #[cfg(target_arch = "x86_64")]
    prev: u32,
    #[cfg(target_arch = "aarch64")]
    prev: u64,
}

/// MXCSR bit 15 — flush-to-zero (FTZ). Anything denormal in an SSE/AVX
/// result becomes a zero with this set.
#[cfg(target_arch = "x86_64")]
const MXCSR_FTZ: u32 = 0x8000;

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn read_mxcsr() -> u32 {
    let mut mxcsr: u32 = 0;
    unsafe {
        std::arch::asm!(
            "stmxcsr [{}]",
            in(reg) &mut mxcsr,
            options(nostack, preserves_flags),
        );
    }
    mxcsr
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn write_mxcsr(mxcsr: u32) {
    unsafe {
        std::arch::asm!(
            "ldmxcsr [{}]",
            in(reg) &mxcsr,
            options(nostack, preserves_flags),
        );
    }
}

impl ScopedFlushToZero {
    #[inline]
    pub fn new() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            let prev = unsafe { read_mxcsr() };
            unsafe { write_mxcsr(prev | MXCSR_FTZ) };
            Self { prev }
        }
        #[cfg(target_arch = "aarch64")]
        {
            // FPCR bit 24 (FZ): flush denormal results to zero.
            let prev: u64;
            unsafe {
                std::arch::asm!("mrs {}, fpcr", out(reg) prev, options(nomem, nostack));
                std::arch::asm!("msr fpcr, {}", in(reg) prev | (1 << 24), options(nomem, nostack));
            }
            Self { prev }
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            Self {}
        }
    }
}

impl Default for ScopedFlushToZero {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ScopedFlushToZero {
    #[inline]
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            write_mxcsr(self.prev);
        }
        #[cfg(target_arch = "aarch64")]
        unsafe {
            std::arch::asm!("msr fpcr, {}", in(reg) self.prev, options(nomem, nostack));
        }
    }
}

/// Per-sample flush-to-zero for `f32` filter/delay feedback state that
/// decays into the denormal range. Complements the thread-wide guard
/// (which not every host honours).
#[inline]
pub fn flush_denormal(x: f32) -> f32 {
    if !x.is_normal() && x != 0.0 { 0.0 } else { x }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_round_trip() {
        // Build a guard, drop it, and check the FP control word is back to its
        // prior value. Exact bit pattern is arch-specific; we just want the
        // before/after to match.
        #[cfg(target_arch = "x86_64")]
        {
            let before = unsafe { read_mxcsr() };
            {
                let _ftz = ScopedFlushToZero::new();
            }
            let after = unsafe { read_mxcsr() };
            assert_eq!(before, after);
        }
        #[cfg(target_arch = "aarch64")]
        {
            let before: u64;
            unsafe {
                std::arch::asm!("mrs {}, fpcr", out(reg) before, options(nomem, nostack));
            }
            {
                let _ftz = ScopedFlushToZero::new();
            }
            let after: u64;
            unsafe {
                std::arch::asm!("mrs {}, fpcr", out(reg) after, options(nomem, nostack));
            }
            assert_eq!(before, after);
        }
    }

    #[test]
    fn flush_denormal_zeroes_subnormals() {
        let tiny = f32::from_bits(1);
        assert_eq!(flush_denormal(tiny), 0.0);
        assert_eq!(flush_denormal(1.0), 1.0);
        assert_eq!(flush_denormal(0.0), 0.0);
        // NaN is not normal but is not denormal either; current impl flushes.
        // Matches vxn-1's behaviour: filter state with NaN is already broken.
        assert_eq!(flush_denormal(f32::NAN), 0.0);
    }
}
