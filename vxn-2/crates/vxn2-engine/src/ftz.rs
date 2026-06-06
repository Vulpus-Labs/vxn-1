//! RAII guard that enables flush-to-zero on the current thread for the
//! lifetime of one audio block, restoring the previous FP control word on
//! drop.
//!
//! Denormal arithmetic costs ~100× and silently wrecks real-time deadlines
//! in filter/delay feedback paths (the FDN reverb and stereo delay are the
//! obvious offenders here). Hold one at the top of every `process()` call.
//!
//! Per-process (not once in `activate`) because the FP control word is
//! thread-local and the host may dispatch `process` on a different thread.
//! Restoring on drop keeps us from perturbing the FP mode the host or
//! downstream plugins see after we return.

#[must_use = "FTZ is restored when the guard drops; bind it for the whole block"]
pub struct ScopedFlushToZero {
    #[cfg(target_arch = "x86_64")]
    prev: u32,
    #[cfg(target_arch = "aarch64")]
    prev: u64,
}

impl ScopedFlushToZero {
    #[inline]
    pub fn new() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            use std::arch::x86_64::{
                _MM_FLUSH_ZERO_ON, _MM_GET_FLUSH_ZERO_MODE, _MM_SET_FLUSH_ZERO_MODE,
            };
            let prev = unsafe { _MM_GET_FLUSH_ZERO_MODE() };
            unsafe { _MM_SET_FLUSH_ZERO_MODE(_MM_FLUSH_ZERO_ON) };
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
            std::arch::x86_64::_MM_SET_FLUSH_ZERO_MODE(self.prev);
        }
        #[cfg(target_arch = "aarch64")]
        unsafe {
            std::arch::asm!("msr fpcr, {}", in(reg) self.prev, options(nomem, nostack));
        }
    }
}
