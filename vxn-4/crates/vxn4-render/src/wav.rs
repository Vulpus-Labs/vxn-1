//! Minimal 16-bit PCM stereo WAV writer.
//!
//! Hand-rolled rather than pulled in as a dependency: this is ~40 lines of
//! header, the workspace has no WAV crate today, and adding one to hear a test
//! tone is a poor trade.
//!
//! 16-bit rather than float because the point is to open the file and listen to
//! it, and 16-bit PCM plays everywhere without asking. The engine's limiter
//! already guarantees |x| <= 1, so the conversion cannot clip on its own — but
//! it saturates anyway rather than wrapping, because a wrap would turn a
//! headroom bug into full-scale noise and hide its own cause.

use std::io::{self, Write};

/// Write interleaved stereo as a 16-bit PCM WAV.
pub fn write_stereo(
    path: &std::path::Path,
    left: &[f32],
    right: &[f32],
    sample_rate: u32,
) -> io::Result<()> {
    assert_eq!(left.len(), right.len());
    let frames = left.len() as u32;
    let channels = 2u16;
    let bits = 16u16;
    let block_align = channels * bits / 8;
    let byte_rate = sample_rate * block_align as u32;
    let data_bytes = frames * block_align as u32;

    let mut w = io::BufWriter::new(std::fs::File::create(path)?);

    w.write_all(b"RIFF")?;
    w.write_all(&(36 + data_bytes).to_le_bytes())?;
    w.write_all(b"WAVE")?;

    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?; // PCM chunk size
    w.write_all(&1u16.to_le_bytes())?; // format = PCM
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&bits.to_le_bytes())?;

    w.write_all(b"data")?;
    w.write_all(&data_bytes.to_le_bytes())?;

    for (l, r) in left.iter().zip(right.iter()) {
        w.write_all(&to_i16(*l).to_le_bytes())?;
        w.write_all(&to_i16(*r).to_le_bytes())?;
    }
    w.flush()
}

/// Saturating float-to-i16. See the module docs on why this saturates.
#[inline]
fn to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * 32767.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_scale_saturates_rather_than_wrapping() {
        assert_eq!(to_i16(1.0), 32767);
        assert_eq!(to_i16(-1.0), -32767);
        assert_eq!(to_i16(4.0), 32767);
        assert_eq!(to_i16(-4.0), -32767);
        assert_eq!(to_i16(0.0), 0);
    }

    #[test]
    fn header_is_the_right_length_for_the_payload() {
        let dir = std::env::temp_dir().join("vxn4_wav_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav");
        let n = 100;
        write_stereo(&path, &vec![0.0; n], &vec![0.0; n], 48_000).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        // 44-byte canonical header + 2 channels * 2 bytes * n frames.
        assert_eq!(bytes.len(), 44 + 4 * n);
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let _ = std::fs::remove_file(&path);
    }
}
