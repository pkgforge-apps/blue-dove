//! Nintendo's DSP ADPCM, and the `.dsp` files that carry it.
//!
//! The codec is an eight-byte frame: one byte naming a coefficient pair and
//! a shift, then fourteen four-bit samples. It is also what BRSTM, BCSTM and
//! BFSTM streams hold, so the frame decoder here is shared with
//! [`super::brstm`].

use std::path::Path;

use super::{Audio, audio, nibble, u16_at, u32_at};
use crate::error::At;

/// Samples a frame holds.
const FRAME_SAMPLES: usize = 14;
/// Bytes a frame takes.
const FRAME_BYTES: usize = 8;

/// Bytes of a `.dsp` file's header.
const HEADER: usize = 0x60;

/// Decodes a `.dsp` file.
pub fn open(path: &Path) -> Result<Audio, String> {
    let data = std::fs::read(path).at(path)?;
    let header =
        Header::parse(&data).ok_or_else(|| format!("{}: not a DSP file", path.display()))?;
    let input = &data[HEADER.min(data.len())..];

    let mut history = vec![header.history; header.channels];
    let mut channels = vec![Vec::new(); header.channels];
    if header.channels == 1 {
        channels[0] = decode(
            input,
            &header.coefficients[0],
            &mut history[0],
            header.samples,
        );
    } else {
        // A stereo stream keeps one channel's frame after the other's, so
        // the frames alternate between the channels.
        let mut channel = 0;
        for frame in input.as_chunks::<FRAME_BYTES>().0 {
            let samples = decode(
                frame,
                &header.coefficients[channel],
                &mut history[channel],
                0,
            );
            channels[channel].extend_from_slice(&samples);
            channel = (channel + 1) % header.channels;
        }
    }

    Ok(audio(&channels, header.rate, header.samples))
}

/// The fields of a `.dsp` header the decoder needs.
struct Header {
    /// Channels in the stream.
    channels: usize,
    /// Samples per second.
    rate: u32,
    /// Samples the header claims.
    samples: usize,
    /// One coefficient table per channel.
    coefficients: [[i16; 16]; 2],
    /// The history the first frame continues from.
    history: [i16; 2],
}

impl Header {
    /// Reads a `.dsp` header, or `None` when the file is not one.
    fn parse(data: &[u8]) -> Option<Self> {
        if data.len() < HEADER {
            return None;
        }
        let samples = u32_at(data, 0, false)? as usize;
        let rate = u32_at(data, 8, false)?;
        if rate == 0 || samples == 0 {
            return None;
        }
        if u16_at(data, 0x0e, false)? != 0 {
            return None;
        }
        let mut coefficients = [[0_i16; 16]; 2];
        for (index, table) in coefficients.iter_mut().enumerate() {
            for (position, value) in table.iter_mut().enumerate() {
                let offset = 0x1c + index * 32 + position * 2;
                *value = u16_at(data, offset, false)? as i16;
            }
        }
        let history = [
            u16_at(data, 0x40, false)? as i16,
            u16_at(data, 0x42, false)? as i16,
        ];
        // The extension records the channel count; files without it are mono.
        let channels = match u16_at(data, 0x4a, false) {
            Some(2) => 2,
            _ => 1,
        };
        Some(Self {
            channels,
            rate,
            samples,
            coefficients,
            history,
        })
    }
}

/// Decodes DSP ADPCM frames, stopping after `samples` values.
///
/// The history carries between calls, so a stream can be decoded frame by
/// frame. A `samples` of zero means the frame's own count.
pub(crate) fn decode(
    input: &[u8],
    coefficients: &[i16; 16],
    history: &mut [i16; 2],
    samples: usize,
) -> Vec<f32> {
    let limit = if samples == 0 {
        input.len() / FRAME_BYTES * FRAME_SAMPLES
    } else {
        samples
    };
    let mut out = Vec::with_capacity(limit);
    let [mut first, mut second] = *history;
    for frame in input.as_chunks::<FRAME_BYTES>().0 {
        if out.len() >= limit {
            break;
        }
        let header = frame[0];
        let index = usize::from(header >> 4) & 0x07;
        let shift = u32::from(header & 0x0f);
        let coefficient0 = i32::from(coefficients[index * 2]);
        let coefficient1 = i32::from(coefficients[index * 2 + 1]);

        for sample in 0..FRAME_SAMPLES {
            let byte = frame[1 + sample / 2];
            let value = nibble(byte, sample % 2 == 0);
            let decoded = ((i32::from(first) * coefficient0 + i32::from(second) * coefficient1)
                >> 11)
                + (value << shift);
            second = first;
            first = decoded.clamp(-32768, 32767) as i16;
            out.push(f32::from(first) / 32768.0);
        }
    }
    *history = [first, second];
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame whose coefficient pair is the first, `(0, 0)`, so the samples
    /// are the nibbles themselves.
    fn frame(samples: &[i32; 14]) -> [u8; 8] {
        let mut frame = [0_u8; 8];
        for (index, sample) in samples.iter().enumerate() {
            let nibble = (*sample as u8) & 0x0f;
            if index % 2 == 0 {
                frame[1 + index / 2] |= nibble << 4;
            } else {
                frame[1 + index / 2] |= nibble;
            }
        }
        frame
    }

    #[test]
    fn a_frame_decodes_to_the_nibbles_it_holds() {
        let samples: [i32; 14] = std::array::from_fn(|index| index as i32 - 7);
        let mut history = [0; 2];
        let out = decode(&frame(&samples), &[0; 16], &mut history, 14);

        assert_eq!(out.len(), 14);
        for (decoded, expected) in out.iter().zip(samples) {
            assert!((decoded * 32768.0 - expected as f32).abs() < 0.5);
        }
    }

    #[test]
    fn a_stream_stops_at_the_sample_count() {
        let mut data = Vec::new();
        for _ in 0..3 {
            data.extend_from_slice(&frame(&[1; 14]));
        }
        let mut history = [0; 2];
        let out = decode(&data, &[0; 16], &mut history, 20);
        assert_eq!(out.len(), 20);
    }

    #[test]
    fn another_format_is_no_dsp() {
        let mut data = vec![0_u8; 0x60];
        assert!(Header::parse(&data).is_none(), "no rate");
        data[8..12].copy_from_slice(&32_000_u32.to_be_bytes());
        data[0..4].copy_from_slice(&1000_u32.to_be_bytes());
        let header = Header::parse(&data).expect("a header with a rate");
        assert_eq!(header.rate, 32_000);
        assert_eq!(header.channels, 1);
    }
}
