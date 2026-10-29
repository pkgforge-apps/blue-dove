//! Decoders for the audio formats video game rips use.
//!
//! These are the formats a game's own assets are stored in, not the ones a
//! desktop player normally meets. Each is a small header over an ADPCM
//! stream, so they decode whole into memory like the rest of [`crate::decode`].
//!
//! The set is the easy half of what rips use: Sony's VAG, CRI's ADX and
//! Nintendo's DSP, BRSTM, BCSTM and BFSTM. Formats that need a chip or a
//! console emulated (SPC, NSF, GBS, VGM, PSF and the rest of the chiptune
//! family) are not supported, and neither is CRI's HCA.

use std::path::Path;

pub mod adx;
pub mod brstm;
pub mod dsp;
pub mod vag;

/// Decoded audio: interleaved samples, channel count and rate.
pub type Audio = (Vec<f32>, u32, u32);

/// Decodes `path` with the game-format decoder its extension names.
///
/// Returns `None` for an extension no decoder here claims, which leaves the
/// file to the other decoders.
pub fn open(path: &Path, extension: &str) -> Option<Result<Audio, String>> {
    let decoder = match extension {
        "vag" | "swag" | "vig" => vag::open,
        "adx" => adx::open,
        "dsp" => dsp::open,
        "brstm" | "bcstm" | "bfstm" => brstm::open,
        _ => return None,
    };
    Some(decoder(path))
}

/// Reads a big- or little-endian field, for the formats that carry a byte
/// order mark.
pub(crate) fn u16_at(data: &[u8], offset: usize, little: bool) -> Option<u16> {
    let bytes: [u8; 2] = data.get(offset..offset + 2)?.try_into().ok()?;
    Some(if little {
        u16::from_le_bytes(bytes)
    } else {
        u16::from_be_bytes(bytes)
    })
}

/// Reads a big- or little-endian field, for the formats that carry a byte
/// order mark.
pub(crate) fn u32_at(data: &[u8], offset: usize, little: bool) -> Option<u32> {
    let bytes: [u8; 4] = data.get(offset..offset + 4)?.try_into().ok()?;
    Some(if little {
        u32::from_le_bytes(bytes)
    } else {
        u32::from_be_bytes(bytes)
    })
}

/// Interleaves per-channel runs of samples into one buffer.
pub(crate) fn interleave(channels: &[Vec<f32>], frames: usize) -> Vec<f32> {
    let mut samples = Vec::with_capacity(frames * channels.len());
    for frame in 0..frames {
        for channel in channels {
            samples.push(channel.get(frame).copied().unwrap_or(0.0));
        }
    }
    samples
}

/// Cuts every channel to the shortest run and to `limit`, then interleaves
/// them.
///
/// A stream cut short leaves its channels of unequal length, and a header's
/// sample count is often rounded up to a whole frame, so both bounds apply
/// before the channels are laid out frame by frame.
pub(crate) fn audio(channels: &[Vec<f32>], rate: u32, limit: usize) -> Audio {
    let frames = channels
        .iter()
        .map(Vec::len)
        .min()
        .unwrap_or_default()
        .min(limit);
    (interleave(channels, frames), channels.len() as u32, rate)
}

/// One four-bit sample, sign-extended.
///
/// A nibble holds its value biased by eight, so taking the bias off and
/// subtracting it again turns it into a signed value.
pub(crate) fn nibble(byte: u8, high: bool) -> i32 {
    let nibble = if high { byte >> 4 } else { byte & 0x0f };
    (i32::from(nibble) ^ 8) - 8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_read_in_the_order_they_are_stored() {
        let data = [0x12, 0x34, 0x56, 0x78];
        assert_eq!(u16_at(&data, 0, false), Some(0x1234));
        assert_eq!(u16_at(&data, 0, true), Some(0x3412));
        assert_eq!(u32_at(&data, 0, false), Some(0x1234_5678));
        assert_eq!(u32_at(&data, 0, true), Some(0x7856_3412));
        assert_eq!(u32_at(&data, 2, false), None, "a short read is no value");
    }

    #[test]
    fn interleaving_puts_a_frame_from_each_channel_in_turn() {
        let samples = interleave(&[vec![1.0, 2.0], vec![3.0, 4.0]], 2);
        assert_eq!(samples, vec![1.0, 3.0, 2.0, 4.0]);
    }
}
