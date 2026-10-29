//! Nintendo's stream containers: BRSTM, BCSTM and BFSTM.
//!
//! All three hold the same DSP ADPCM stream as [`super::dsp`], cut into
//! blocks with one block per channel in turn, and carry the predictor history
//! each block starts from. BRSTM is the Wii and GameCube container, BCSTM the
//! 3DS one and BFSTM the Wii U and Switch one.

use std::path::Path;

use super::dsp;
use super::{Audio, u16_at, u32_at};
use crate::error::At;

/// Bytes the DATA chunk spends on its own header before the first block.
const DATA_HEADER: usize = 0x20;

/// The codec a stream holds.
#[derive(Clone, Copy, PartialEq)]
enum Codec {
    /// Signed 8-bit PCM.
    Pcm8,
    /// Signed 16-bit PCM, in the file's byte order.
    Pcm16,
    /// DSP ADPCM.
    Dsp,
}

/// What the decoder needs from a stream's header.
struct Stream {
    /// Whether the file stores its fields little-endian.
    little: bool,
    /// What the blocks hold.
    codec: Codec,
    /// Channels in the stream.
    channels: usize,
    /// Samples per second.
    rate: u32,
    /// Samples in the whole stream.
    samples: usize,
    /// Byte offset of the first block.
    start: usize,
    /// Bytes one channel's block takes.
    block_size: usize,
    /// Samples a full block holds.
    samples_per_block: usize,
    /// Blocks in the stream.
    block_count: usize,
    /// Bytes of the last block that hold audio.
    last_used: usize,
    /// Samples the last block holds.
    last_samples: usize,
    /// Bytes the last block takes, padding included.
    last_size: usize,
    /// One coefficient table per channel.
    coefficients: Vec<[i16; 16]>,
    /// The history the first block starts from, one entry per channel.
    history: Vec<[i16; 2]>,
}

/// Decodes a BRSTM, BCSTM or BFSTM file.
pub fn open(path: &Path) -> Result<Audio, String> {
    let data = std::fs::read(path).at(path)?;
    let stream = match data.get(..4) {
        Some(b"RSTM") => brstm(&data),
        Some(b"FSTM") | Some(b"CSTM") => fstm(&data),
        _ => None,
    }
    .ok_or_else(|| format!("{}: not a BRSTM, BCSTM or BFSTM file", path.display()))?;

    if stream.channels == 0 || stream.channels > 16 {
        return Err(format!("{}: unusable channel count", path.display()));
    }

    let mut samples = vec![0.0_f32; stream.samples * stream.channels];
    let mut history = stream.history.clone();
    if history.len() < stream.channels {
        history.resize(stream.channels, [0, 0]);
    }
    let mut offset = stream.start;
    let mut frames = 0;
    'blocks: for block in 0..stream.block_count {
        let last = block + 1 == stream.block_count;
        let used = if last {
            stream.last_used
        } else {
            stream.block_size
        };
        let count = if last {
            stream.last_samples
        } else {
            stream.samples_per_block
        };
        for (channel, history) in history.iter_mut().enumerate().take(stream.channels) {
            let Some(input) = data.get(offset..offset + used) else {
                break 'blocks;
            };
            let decoded = decode_block(input, &stream, channel, history, count);
            let base = block * stream.samples_per_block;
            for (index, value) in decoded.iter().enumerate() {
                if let Some(slot) = samples.get_mut((base + index) * stream.channels + channel) {
                    *slot = *value;
                }
            }
            offset += if last {
                stream.last_size.max(used)
            } else {
                stream.block_size
            };
        }
        frames = block * stream.samples_per_block + count;
    }

    // A rip can be cut short; the header still claims its full length, so
    // the samples past what the file holds are dropped.
    samples.truncate(frames.min(stream.samples) * stream.channels);
    Ok((samples, stream.channels as u32, stream.rate))
}

/// Decodes one channel's block.
fn decode_block(
    input: &[u8],
    stream: &Stream,
    channel: usize,
    history: &mut [i16; 2],
    count: usize,
) -> Vec<f32> {
    match stream.codec {
        Codec::Dsp => dsp::decode(input, &stream.coefficients[channel], history, count),
        Codec::Pcm16 => input
            .as_chunks::<2>()
            .0
            .iter()
            .take(count)
            .map(|pair| {
                let value = if stream.little {
                    i16::from_le_bytes(*pair)
                } else {
                    i16::from_be_bytes(*pair)
                };
                f32::from(value) / 32768.0
            })
            .collect(),
        Codec::Pcm8 => input
            .iter()
            .take(count)
            .map(|byte| f32::from(*byte as i8) / 128.0)
            .collect(),
    }
}

/// Reads a BRSTM header.
fn brstm(data: &[u8]) -> Option<Stream> {
    let little = u16_at(data, 4, false)? == 0xFFFE;
    let version = u16_at(data, 6, false)?;
    let head = if version == 1 {
        u16_at(data, 0x0c, little)? as usize
    } else {
        u32_at(data, 0x10, little)? as usize
    };
    if data.get(head..head + 4)? != b"HEAD" {
        return None;
    }
    let head_size = u32_at(data, head + 4, little)? as usize;
    let info = if version == 1 {
        head + 0x08
    } else {
        head + 0x20
    };

    let codec = match *data.get(info)? {
        0 => Codec::Pcm8,
        1 => Codec::Pcm16,
        2 => Codec::Dsp,
        _ => return None,
    };
    let channels = usize::from(*data.get(info + 2)?);
    let rate = u32::from(u16_at(data, info + 4, little)?);
    let samples = u32_at(data, info + 0x0c, little)? as usize;
    let start = u32_at(data, info + 0x10, little)? as usize;
    let block_count = u32_at(data, info + 0x14, little)? as usize;
    let block_size = u32_at(data, info + 0x18, little)? as usize;
    let samples_per_block = u32_at(data, info + 0x1c, little)? as usize;
    let last_used = u32_at(data, info + 0x20, little)? as usize;
    let last_samples = u32_at(data, info + 0x24, little)? as usize;
    let last_size = u32_at(data, info + 0x28, little)? as usize;

    let coefficients = if codec == Codec::Dsp {
        let mut tables = Vec::with_capacity(channels);
        if version == 1 {
            for channel in 0..channels {
                let offset = head + 0x38 + channel * 0x30;
                tables.push(coefficients_at(data, offset, little)?);
            }
        } else {
            let part = u32_at(data, head + 0x1c, little)? as usize;
            for channel in 0..channels {
                let entry = head + 0x08 + part + 0x04 + channel * 0x08 + 0x04;
                let offset = head + 0x08 + u32_at(data, entry, little)? as usize + 0x08;
                tables.push(coefficients_at(data, offset, little)?);
            }
        }
        tables
    } else {
        Vec::new()
    };

    let history = if codec == Codec::Dsp {
        history_table(data, head + head_size, channels, little, true)
    } else {
        Vec::new()
    };

    Some(Stream {
        little,
        codec,
        channels,
        rate,
        samples,
        start,
        block_size,
        samples_per_block,
        block_count,
        last_used,
        last_samples,
        last_size,
        coefficients,
        history,
    })
}

/// Reads a BCSTM or BFSTM header.
fn fstm(data: &[u8]) -> Option<Stream> {
    let little = u16_at(data, 4, false)? == 0xFFFE;
    let sections = usize::from(u16_at(data, 0x10, little)?);
    let mut info = 0;
    let mut seek = 0;
    let mut data_start = 0;
    for index in 0..sections {
        let entry = 0x14 + index * 0x0c;
        let offset = u32_at(data, entry + 4, little)? as usize;
        match u16_at(data, entry, little)? {
            0x4000 => info = offset,
            0x4001 => seek = offset,
            0x4002 => data_start = offset,
            _ => {}
        }
    }
    if info == 0 || data_start == 0 || data.get(info..info + 4)? != b"INFO" {
        return None;
    }

    let codec = match *data.get(info + 0x20)? {
        0 => Codec::Pcm8,
        1 => Codec::Pcm16,
        2 => Codec::Dsp,
        _ => return None,
    };
    let channels = usize::from(*data.get(info + 0x22)?);
    let rate = u32_at(data, info + 0x24, little)?;
    let samples = u32_at(data, info + 0x2c, little)? as usize;
    let block_count = u32_at(data, info + 0x30, little)? as usize;
    let block_size = u32_at(data, info + 0x34, little)? as usize;
    let samples_per_block = u32_at(data, info + 0x38, little)? as usize;
    let last_used = u32_at(data, info + 0x3c, little)? as usize;
    let last_samples = u32_at(data, info + 0x40, little)? as usize;
    let last_size = u32_at(data, info + 0x44, little)? as usize;

    let coefficients = if codec == Codec::Dsp {
        let table = info + 0x08 + u32_at(data, info + 0x1c, little)? as usize;
        let mut tables = Vec::with_capacity(channels);
        for channel in 0..channels {
            let entry = table + 0x04 + channel * 0x08 + 0x04;
            let channel_info = table + u32_at(data, entry, little)? as usize;
            let offset = channel_info + u32_at(data, channel_info + 0x04, little)? as usize;
            tables.push(coefficients_at(data, offset, little)?);
        }
        tables
    } else {
        Vec::new()
    };

    let history = if codec == Codec::Dsp {
        history_table(data, seek, channels, little, false)
    } else {
        Vec::new()
    };

    Some(Stream {
        little,
        codec,
        channels,
        rate,
        samples,
        start: data_start + DATA_HEADER,
        block_size,
        samples_per_block,
        block_count,
        last_used,
        last_samples,
        last_size,
        coefficients,
        history,
    })
}

/// Reads one channel's sixteen coefficients.
fn coefficients_at(data: &[u8], offset: usize, little: bool) -> Option<[i16; 16]> {
    let mut table = [0_i16; 16];
    for (index, value) in table.iter_mut().enumerate() {
        *value = u16_at(data, offset + index * 2, little)? as i16;
    }
    Some(table)
}

/// Reads the history the first block starts from.
///
/// The table holds one entry per block and channel, in the order the blocks
/// are; only the first block's entries are used, since the rest of the stream
/// carries its state forward and the table's own values are rounded.
fn history_table(
    data: &[u8],
    from: usize,
    count: usize,
    little: bool,
    walk: bool,
) -> Vec<[i16; 2]> {
    let mut offset = from;
    if walk {
        // BRSTM lists its chunks after HEAD, each with a size that counts its
        // own header.
        loop {
            let Some(tag) = data.get(offset..offset + 4) else {
                return Vec::new();
            };
            if tag == b"ADPC" {
                offset += 8;
                break;
            }
            if tag == b"DATA" {
                return Vec::new();
            }
            match u32_at(data, offset + 4, little) {
                Some(size) if size >= 8 => offset += size as usize,
                _ => return Vec::new(),
            }
        }
    } else {
        // BCSTM and BFSTM point straight at the SEEK section, whose entries
        // are little-endian even in a big-endian file.
        if data.get(offset..offset + 4) != Some(b"SEEK") {
            return Vec::new();
        }
        offset += 8;
        return (0..count)
            .filter_map(|index| {
                let first = u16_at(data, offset + index * 4, true)? as i16;
                let second = u16_at(data, offset + index * 4 + 2, true)? as i16;
                Some([first, second])
            })
            .collect();
    }

    (0..count)
        .filter_map(|index| {
            let first = u16_at(data, offset + index * 4, little)? as i16;
            let second = u16_at(data, offset + index * 4 + 2, little)? as i16;
            Some([first, second])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coefficient_table_reads_in_the_file_order() {
        let mut data = vec![0_u8; 32];
        data[0..2].copy_from_slice(&100_i16.to_be_bytes());
        data[2..4].copy_from_slice(&(-200_i16).to_be_bytes());
        let table = coefficients_at(&data, 0, false).expect("the table reads");
        assert_eq!(table[0], 100);
        assert_eq!(table[1], -200);
    }

    #[test]
    fn another_format_is_no_stream() {
        assert!(brstm(b"RIFF\x00\x00\x00\x00WAVE").is_none());
        assert!(fstm(b"RIFF\x00\x00\x00\x00WAVE").is_none());
    }
}
