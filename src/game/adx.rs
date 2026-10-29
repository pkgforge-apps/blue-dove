//! CRI's ADX format, used by Sega, Atlus and many PlayStation 2 rips.
//!
//! Each frame holds 32 four-bit samples per channel, scaled by a value the
//! frame carries and filtered through a second-order predictor worked out
//! from the header's high-pass cutoff.

use std::f64::consts::{PI, SQRT_2};
use std::path::Path;

use super::{Audio, audio, nibble, u16_at, u32_at};
use crate::error::At;

/// Samples a frame holds.
const FRAME_SAMPLES: usize = 32;
/// Bytes a frame takes per channel.
const FRAME_BYTES: usize = 18;

/// Decodes an ADX file.
pub fn open(path: &Path) -> Result<Audio, String> {
    let data = std::fs::read(path).at(path)?;
    let header = header(&data).ok_or_else(|| format!("{}: not an ADX file", path.display()))?;

    let mut channels = vec![Vec::new(); header.channels];
    let mut history = vec![[0_i32; 2]; header.channels];
    let frame_size = FRAME_BYTES * header.channels;

    'frames: for frame in data[header.start..].chunks_exact(frame_size) {
        for (index, block) in frame.as_chunks::<FRAME_BYTES>().0.iter().enumerate() {
            let scale = i32::from(i16::from_be_bytes([block[0], block[1]]));
            // A frame whose scale has its top bit set ends the stream.
            if scale & 0x8000 != 0 {
                break 'frames;
            }
            let [mut first, mut second] = history[index];
            for sample in 0..FRAME_SAMPLES {
                let byte = block[2 + sample / 2];
                let value = nibble(byte, sample % 2 == 0);
                let decoded = value * scale
                    + ((header.coefficients[0] * first + header.coefficients[1] * second) >> 12);
                second = first;
                first = decoded.clamp(-32768, 32767);
                channels[index].push(first as f32 / 32768.0);
            }
            history[index] = [first, second];
        }
    }

    Ok(audio(&channels, header.rate, header.samples))
}

/// The fields of an ADX header the decoder needs.
struct Header {
    /// Byte offset of the first frame.
    start: usize,
    /// Channels in the stream.
    channels: usize,
    /// Samples per second.
    rate: u32,
    /// Samples the header claims, used to trim the last frame.
    samples: usize,
    /// The predictor pair, in 4096ths.
    coefficients: [i32; 2],
}

/// Reads an ADX header, or `None` when the file is something else.
fn header(data: &[u8]) -> Option<Header> {
    if u16_at(data, 0, false)? != 0x8000 {
        return None;
    }
    // Only the 4-bit, 18-byte, 4-bit-depth variant is in use.
    if data.get(4..7)? != [3, FRAME_BYTES as u8, 4] {
        return None;
    }
    let channels = usize::from(*data.get(7)?);
    if channels == 0 || channels > 8 {
        return None;
    }
    let start = usize::from(u16_at(data, 2, false)?) + 4;
    let rate = u32_at(data, 8, false)?;
    if rate == 0 {
        return None;
    }
    let samples = u32_at(data, 12, false)? as usize;
    let cutoff = u16_at(data, 16, false)?;
    Some(Header {
        start,
        channels,
        rate,
        samples,
        coefficients: coefficients(cutoff, rate),
    })
}

/// The second-order predictor for `cutoff` at `rate`, in 4096ths.
fn coefficients(cutoff: u16, rate: u32) -> [i32; 2] {
    let a = SQRT_2 - (2.0 * PI * f64::from(cutoff) / f64::from(rate)).cos();
    let b = SQRT_2 - 1.0;
    let c = (a - ((a + b) * (a - b)).sqrt()) / b;
    [
        (c * 2.0 * 4096.0).round_ties_even() as i32,
        (-(c * c) * 4096.0).round_ties_even() as i32,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a mono ADX header over `frames`.
    fn adx(rate: u32, samples: usize, frames: &[u8]) -> Vec<u8> {
        let mut data = vec![0_u8; 0x20];
        data[0..2].copy_from_slice(&0x8000_u16.to_be_bytes());
        data[2..4].copy_from_slice(&0x1c_u16.to_be_bytes());
        data[4] = 3;
        data[5] = 0x12;
        data[6] = 4;
        data[7] = 1;
        data[8..12].copy_from_slice(&rate.to_be_bytes());
        data[12..16].copy_from_slice(&(samples as u32).to_be_bytes());
        data[16..18].copy_from_slice(&500_u16.to_be_bytes());
        data.extend_from_slice(frames);
        data
    }

    #[test]
    fn the_header_gives_the_rate_and_the_frame_start() {
        let header = header(&adx(44_100, 100, &[])).expect("the header parses");
        assert_eq!(header.rate, 44_100);
        assert_eq!(header.channels, 1);
        assert_eq!(header.start, 0x20);
        assert_eq!(header.samples, 100);
    }

    #[test]
    fn the_predictor_falls_as_the_cutoff_rises() {
        // The cutoff is the high-pass corner, so raising it hands less of the
        // previous samples to the predictor and both coefficients shrink.
        let low = coefficients(500, 44_100);
        let high = coefficients(4000, 44_100);
        assert_eq!(low, [7334, -3283], "the reference coefficients for 500 Hz");
        assert!(low[0] > high[0]);
        assert!(low[1] < high[1], "the second coefficient is negative");
    }

    #[test]
    fn another_format_is_no_adx() {
        assert!(header(b"RIFF\x00\x00\x00\x00WAVE").is_none());
        assert!(header(&[0x80, 0x00, 0x00]).is_none(), "a short header");
    }
}
