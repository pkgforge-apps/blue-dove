//! Sony's VAG format, the PlayStation ADPCM stream.
//!
//! VAG is the PlayStation SDK's stream container: a header, then 16-byte
//! frames that each hold 28 four-bit samples. Most PlayStation, PS2 and PSP
//! rips use it.

use std::path::Path;

use super::{Audio, audio, nibble, u32_at};
use crate::error::At;

/// Samples in a frame: fourteen bytes of nibbles.
const FRAME_SAMPLES: usize = 28;

/// The predictor pairs the format allows, in sixty-fourths.
const COEFFICIENTS: [[i32; 2]; 5] = [[0, 0], [60, 0], [115, -52], [98, -55], [122, -60]];

/// Decodes a VAG file.
pub fn open(path: &Path) -> Result<Audio, String> {
    let data = std::fs::read(path).at(path)?;
    let (layout, rate) =
        layout(&data).ok_or_else(|| format!("{}: not a VAG file", path.display()))?;
    if rate == 0 {
        return Err(format!("{}: no sample rate in the header", path.display()));
    }

    let mut history = vec![[0_i32; 2]; layout.channels];
    let mut channels = vec![Vec::new(); layout.channels];
    let mut offset = layout.start;
    let mut channel = 0;
    let mut size = layout.first;
    while offset < data.len() {
        let end = (offset + size).min(data.len());
        decode(
            &data[offset..end],
            &mut history[channel],
            &mut channels[channel],
        );
        offset = end;
        channel = (channel + 1) % layout.channels;
        size = layout.interleave;
    }

    Ok(audio(&channels, rate, usize::MAX))
}

/// Where the audio starts and how its channels are laid out.
struct Layout {
    /// Byte offset of the first frame.
    start: usize,
    /// Channels in the stream.
    channels: usize,
    /// Bytes of one channel's run before the next channel's starts.
    interleave: usize,
    /// Bytes of the first channel's run, when the header sits inside it.
    first: usize,
}

/// Reads the header, returning the layout and the sample rate.
fn layout(data: &[u8]) -> Option<(Layout, u32)> {
    let magic = data.get(..4)?;
    let big = match magic {
        b"VAGp" | b"VAGi" | b"VAG1" | b"VAG2" => true,
        b"pGAV" => false,
        _ => return None,
    };
    let version = u32_at(data, 4, !big)?;
    let rate = u32_at(data, 0x10, !big)?;

    if magic == b"VAGi" {
        let interleave = u32_at(data, 8, true)? as usize;
        return Some((
            Layout {
                start: 0x800,
                channels: 2,
                interleave,
                first: interleave,
            },
            rate,
        ));
    }
    if magic == b"VAG1" || magic == b"VAG2" {
        let channels = if magic == b"VAG2" {
            2
        } else {
            usize::from(*data.get(0x1e)?).max(1)
        };
        let interleave = if magic == b"VAG2" { 0x800 } else { 0x10 };
        return Some((
            Layout {
                start: 0x40,
                channels,
                interleave,
                first: interleave,
            },
            rate,
        ));
    }

    // A second header inside the file means the channels are stored in
    // blocks, each block starting with its own copy of the header.
    for block in [0x1000, 0x2000, 0x6000, 0x800] {
        if data.get(block..block + 4) == Some(b"VAGp") {
            return Some((
                Layout {
                    start: 0x30,
                    channels: 2,
                    interleave: block,
                    first: block - 0x30,
                },
                rate,
            ));
        }
    }

    // The later revisions name the channel count where the older ones keep
    // the name field; an older header has zero there, which reads as mono.
    let named = matches!(version, 0x0002_0001 | 0x0003_0000 | 0x0000_0004);
    let channels = match data.get(0x1e).copied() {
        Some(count) if named && (1..=8).contains(&count) => usize::from(count),
        _ => 1,
    };
    let start = if version == 4 { 0x60 } else { 0x30 };
    Some((
        Layout {
            start,
            channels,
            interleave: 0x10,
            first: 0x10,
        },
        rate,
    ))
}

/// Appends the samples of every whole frame in `data`.
fn decode(data: &[u8], history: &mut [i32; 2], out: &mut Vec<f32>) {
    let [mut first, mut second] = *history;
    for frame in data.as_chunks::<16>().0 {
        let header = frame[0];
        let shift = i32::from(header & 0x0f);
        let filter = usize::from(header >> 4);
        let [coefficient0, coefficient1] =
            COEFFICIENTS.get(filter).copied().unwrap_or(COEFFICIENTS[0]);
        let muted = frame[1] & 0x07 == 0x07;

        for index in 0..FRAME_SAMPLES {
            let byte = frame[2 + index / 2];
            let value = nibble(byte, index % 2 == 1);
            let sample = if muted {
                0
            } else {
                ((value << 12) >> shift) + (first * coefficient0 + second * coefficient1) / 64
            };
            second = first;
            first = sample;
            out.push(sample.clamp(-32768, 32767) as f32 / 32768.0);
        }
    }
    *history = [first, second];
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a VAG header over `frames`, mono unless `channels` says two.
    fn vag(rate: u32, channels: u8, frames: &[u8]) -> Vec<u8> {
        let mut data = vec![0_u8; 0x30];
        data[..4].copy_from_slice(b"VAGp");
        data[4..8].copy_from_slice(&0x20_u32.to_be_bytes());
        data[0x0c..0x10].copy_from_slice(&(frames.len() as u32).to_be_bytes());
        data[0x10..0x14].copy_from_slice(&rate.to_be_bytes());
        if channels == 2 {
            // The later revisions name the channel count where the older ones
            // keep the stream name.
            data[4..8].copy_from_slice(&0x0002_0001_u32.to_be_bytes());
            data[0x1e] = 2;
        }
        data.extend_from_slice(frames);
        data
    }

    /// A frame whose nibbles are the samples themselves: filter 0, shift 12.
    fn frame(samples: &[i32; 28]) -> [u8; 16] {
        let mut frame = [0_u8; 16];
        frame[0] = 12;
        for (index, sample) in samples.iter().enumerate() {
            let nibble = (*sample as u8) & 0x0f;
            if index % 2 == 0 {
                frame[2 + index / 2] |= nibble;
            } else {
                frame[2 + index / 2] |= nibble << 4;
            }
        }
        frame
    }

    #[test]
    fn a_frame_decodes_to_the_nibbles_it_holds() {
        let samples: [i32; 28] = std::array::from_fn(|index| (index % 16) as i32 - 8);
        let mut history = [0; 2];
        let mut out = Vec::new();
        decode(&frame(&samples), &mut history, &mut out);

        assert_eq!(out.len(), 28);
        for (decoded, expected) in out.iter().zip(samples) {
            assert!((decoded * 32768.0 - expected as f32).abs() < 0.5);
        }
    }

    #[test]
    fn the_header_gives_the_rate_and_the_channel_count() {
        let (mono, rate) = layout(&vag(22_050, 1, &[])).expect("the header parses");
        assert_eq!(rate, 22_050);
        assert_eq!(mono.channels, 1);
        assert_eq!(mono.start, 0x30);

        let (stereo, _) = layout(&vag(22_050, 2, &[])).expect("the header parses");
        assert_eq!(stereo.channels, 2);
    }

    #[test]
    fn another_format_is_no_vag() {
        assert!(layout(b"RIFF\x00\x00\x00\x00WAVE").is_none());
    }

    #[test]
    fn a_mono_file_decodes_every_frame() {
        let samples: [i32; 28] = std::array::from_fn(|index| (index % 16) as i32 - 8);
        let mut frames = Vec::new();
        frames.extend_from_slice(&frame(&samples));
        frames.extend_from_slice(&frame(&samples));
        let path =
            std::env::temp_dir().join(format!("blue-dove-vag-mono-{}.vag", std::process::id()));
        std::fs::write(&path, vag(22_050, 1, &frames)).expect("the file is written");
        let (decoded, channels, rate) = open(&path).expect("the file decodes");
        let _ = std::fs::remove_file(&path);

        assert_eq!((channels, rate), (1, 22_050));
        assert_eq!(decoded.len(), 2 * FRAME_SAMPLES, "both frames are decoded");
        for frame in decoded.as_chunks::<FRAME_SAMPLES>().0 {
            for (decoded, expected) in frame.iter().zip(samples) {
                assert!((decoded * 32768.0 - expected as f32).abs() < 0.5);
            }
        }
    }
}
