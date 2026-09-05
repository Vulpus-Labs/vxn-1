//! Minimal in-place radix-2 FFT, for the `alias` analysis binary.
//!
//! Hand-rolled for the same reason as the WAV writer: it is short, the
//! workspace has no FFT dependency, and adding one to answer a sizing question
//! is a poor trade. Analysis only — never on the audio thread.

use std::f32::consts::PI;

/// In-place complex FFT. `re` and `im` must be the same power-of-two length.
pub fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    assert_eq!(n, im.len());
    assert!(n.is_power_of_two(), "fft length {n} is not a power of two");

    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let mut len = 2;
    while len <= n {
        let ang = -2.0 * PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let (tr, ti) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
        }
        len <<= 1;
    }
}

/// Hann window, for leakage control.
pub fn hann(n: usize) -> Vec<f32> {
    (0..n)
        .map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / n as f32).cos())
        .collect()
}

/// Magnitude spectrum of a real signal, first `n/2` bins.
pub fn spectrum(x: &[f32]) -> Vec<f32> {
    let n = x.len().next_power_of_two().min(x.len());
    let n = if n.is_power_of_two() {
        n
    } else {
        n.next_power_of_two() >> 1
    };
    let w = hann(n);
    let mut re: Vec<f32> = x[..n].iter().zip(&w).map(|(v, k)| v * k).collect();
    let mut im = vec![0.0f32; n];
    fft(&mut re, &mut im);
    (0..n / 2)
        .map(|i| (re[i] * re[i] + im[i] * im[i]).sqrt())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pure_tone_lands_in_one_bin() {
        let n = 1024;
        let bin = 64;
        let x: Vec<f32> = (0..n)
            .map(|i| (2.0 * PI * bin as f32 * i as f32 / n as f32).sin())
            .collect();
        let s = spectrum(&x);
        let peak = s
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert_eq!(peak, bin, "energy landed in bin {peak}, expected {bin}");
        // Hann spreads over 3 bins; everything else must be far down.
        let far: f32 = s
            .iter()
            .enumerate()
            .filter(|(i, _)| i.abs_diff(bin) > 2)
            .map(|(_, v)| *v)
            .fold(0.0, f32::max);
        assert!(far < s[bin] * 0.01, "leakage {far} vs peak {}", s[bin]);
    }

    #[test]
    fn dc_lands_in_bin_zero() {
        let x = vec![1.0f32; 256];
        let s = spectrum(&x);
        assert!(s[0] > s[1..].iter().cloned().fold(0.0, f32::max));
    }
}
