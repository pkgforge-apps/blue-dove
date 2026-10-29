//! Opening an audio file with the decoder its format needs.
//!
//! rodio decodes through symphonia, which covers the common formats. The ones
//! it cannot open get a pure Rust decoder of their own here: Monkey's Audio,
//! WavPack, TTA, Musepack and Shorten. Those decode whole into memory, which
//! keeps seeking an index into a buffer and takes all I/O off the audio
//! thread; a track is a few megabytes of PCM at most.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Duration;

use rodio::source::{SeekError, Source};
use rodio::{ChannelCount, Decoder, SampleRate};

use crate::error::At;

/// Opens `path` as a source of interleaved f32 samples.
///
/// The extension picks the decoder. Opus has its own Ogg reader, the formats
/// symphonia lacks get a whole file decode, and everything else goes through
/// symphonia. A source keeps its own rate and channel count; rodio's mixer
/// resamples it to the output.
pub fn open(path: &Path) -> Result<Box<dyn Source + Send>, String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();

    match extension.as_str() {
        "opus" => Ok(Box::new(crate::opus::OpusFile::open(path)?)),
        "ape" => Ok(Box::new(Pcm::ape(path)?)),
        "wv" => Ok(Box::new(Pcm::wavpack(path)?)),
        "tta" => Ok(Box::new(Pcm::tta(path)?)),
        "mpc" | "mpp" | "mp+" => Ok(Box::new(Pcm::musepack(path)?)),
        "shn" => Ok(Box::new(Pcm::shorten(path)?)),
        _ => {
            // The game formats a rip is stored in have their own decoders.
            if let Some(decoded) = crate::game::open(path, &extension) {
                let (samples, channels, rate) = decoded?;
                return Ok(Box::new(Pcm::wrap(samples, channels, rate, path)?));
            }
            let file = File::open(path).at(path)?;
            // `try_from` hands the decoder the file's length, which symphonia
            // needs to seek backwards. `Decoder::new` leaves it out, and then
            // only a forward seek works.
            let decoder = Decoder::try_from(file).at(path)?;
            Ok(Box::new(decoder))
        }
    }
}

/// A whole file decoded to interleaved samples at its own rate.
pub struct Pcm {
    /// Interleaved samples, every channel per frame.
    samples: Vec<f32>,
    /// Channels in the stream.
    channels: u32,
    /// The stream's own rate, which rodio's mixer resamples as needed.
    rate: u32,
    /// How many samples have been handed out.
    position: usize,
}

impl Pcm {
    /// Decodes a Monkey's Audio file.
    fn ape(path: &Path) -> Result<Self, String> {
        let file = File::open(path).at(path)?;
        let mut decoder = ape_decoder::ApeDecoder::new(BufReader::new(file)).at(path)?;
        let info = decoder.info().clone();
        let mut pcm = decoder.decode_all().at(path)?;

        // The decoder hands back the source's own byte order, and leaves the
        // sample layout of a non-WAV source alone, so a big-endian stream has
        // to be turned round here.
        let width = usize::from(info.bytes_per_sample);
        if info.is_big_endian && width > 1 {
            for sample in pcm.chunks_exact_mut(width) {
                sample.reverse();
            }
        }

        let mut samples = Vec::with_capacity(pcm.len() / width.max(1));
        if info.is_floating_point {
            push_float(&pcm, &mut samples);
        } else if info.bits_per_sample == 8 && !info.is_signed_8bit {
            push_unsigned_8(&pcm, &mut samples);
        } else {
            push_signed(&pcm, width, &mut samples);
        }
        Self::wrap(samples, u32::from(info.channels), info.sample_rate, path)
    }

    /// Decodes a WavPack file.
    ///
    /// `wavicle` covers integer PCM in mono and stereo; a hybrid file with a
    /// correction file, DSD and more than two channels are out of its scope
    /// and come back as an error.
    fn wavpack(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).at(path)?;
        let stream = wavicle::decode_stream(without_ape_tag(&data)).at(path)?;
        let samples = if stream.is_float {
            stream
                .samples
                .iter()
                .map(|sample| f32::from_bits(*sample as u32))
                .collect()
        } else {
            let scale = scale(stream.bits_per_sample);
            stream
                .samples
                .iter()
                .map(|sample| *sample as f32 * scale)
                .collect()
        };
        Self::wrap(samples, stream.channels, stream.sample_rate, path)
    }

    /// Decodes a TTA file.
    fn tta(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).at(path)?;
        let (header, pcm) = oxideav_tta::decode(&data).at(path)?;
        let scale = scale(u32::from(header.bits_per_sample));
        let samples = pcm.iter().map(|sample| *sample as f32 * scale).collect();
        Self::wrap(
            samples,
            u32::from(header.channels),
            header.sample_rate,
            path,
        )
    }

    /// Decodes a Musepack file, in either the SV7 or the SV8 generation.
    fn musepack(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).at(path)?;
        let stream = oxideav_musepack::mpc_decode::decode_mpc_stream_tagged(&data).at(path)?;
        let rate = stream
            .sample_rate_hz()
            .ok_or_else(|| format!("{}: unknown sample rate", path.display()))?;
        // The decoder works in the 16-bit domain throughout.
        let samples = stream
            .pcm()
            .iter()
            .map(|sample| (*sample / 32_768.0) as f32)
            .collect();
        Self::wrap(samples, u32::from(stream.channels()), rate, path)
    }

    /// Decodes a Shorten file.
    ///
    /// The Shorten header carries no sample rate: the stream quotes the
    /// original file's header verbatim instead, and that is where the rate
    /// comes from.
    fn shorten(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path).at(path)?;
        let stream = oxideav_shorten::decode_stream(&data).at(path)?;
        let rate = envelope_rate(&stream.verbatim)
            .ok_or_else(|| format!("{}: no sample rate in the file header", path.display()))?;
        let unsigned = matches!(
            stream.header.filetype_pinned(),
            Some(oxideav_shorten::Filetype::U8)
        );
        let bits = if unsigned { 8 } else { 16 };
        let scale = scale(bits);

        let channels = stream.channels.len();
        let frames = stream
            .channels
            .iter()
            .map(Vec::len)
            .min()
            .unwrap_or_default();
        let mut samples = Vec::with_capacity(frames * channels);
        for frame in 0..frames {
            for channel in &stream.channels {
                let sample = channel[frame] as f32 * scale;
                // An 8-bit WAV is unsigned, and the codec keeps that offset.
                samples.push(if unsigned { sample - 1.0 } else { sample });
            }
        }
        Self::wrap(samples, channels as u32, rate, path)
    }

    /// Wraps decoded samples, rejecting a stream that holds no audio.
    fn wrap(samples: Vec<f32>, channels: u32, rate: u32, path: &Path) -> Result<Self, String> {
        if samples.is_empty() {
            return Err(format!("{}: no audio in the file", path.display()));
        }
        if channels == 0 || rate == 0 {
            return Err(format!("{}: unusable stream format", path.display()));
        }
        Ok(Self {
            samples,
            channels,
            rate,
            position: 0,
        })
    }

    /// Frames in the stream, for the duration.
    fn frames(&self) -> usize {
        self.samples.len() / self.channels as usize
    }
}

impl Iterator for Pcm {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        let sample = self.samples.get(self.position).copied();
        self.position += 1;
        sample
    }
}

impl Source for Pcm {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.samples.len() - self.position.min(self.samples.len()))
    }

    fn channels(&self) -> ChannelCount {
        let channels: u16 = self
            .channels
            .try_into()
            .expect("a stream has at most 65535 channels");
        ChannelCount::new(channels).expect("a wrapped stream has at least one channel")
    }

    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(self.rate).expect("a wrapped stream has a non-zero rate")
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            self.frames() as f64 / f64::from(self.rate),
        ))
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        let frame = (position.as_secs_f64() * f64::from(self.rate)) as usize;
        let frame = frame.min(self.frames());
        self.position = frame * self.channels as usize;
        Ok(())
    }
}

/// One over the full-scale value of a signed sample of `bits` bits.
fn scale(bits: u32) -> f32 {
    1.0 / (1u64 << (bits.clamp(1, 32) - 1)) as f32
}

/// Drops a trailing APEv2 tag, which encoders write after a WavPack stream.
///
/// `wavicle` reads blocks until the data runs out and stops at the first byte
/// that is not a block, so a tag has to come off first. A tag ends with a
/// 32-byte footer that gives the length of the items before it.
fn without_ape_tag(data: &[u8]) -> &[u8] {
    let Some(tail) = data.len().checked_sub(32) else {
        return data;
    };
    if &data[tail..tail + 8] != b"APETAGEX" {
        return data;
    }
    let size = u32::from_le_bytes(data[tail + 12..tail + 16].try_into().unwrap_or_default());
    let flags = u32::from_le_bytes(data[tail + 20..tail + 24].try_into().unwrap_or_default());
    // The size counts the items and the footer, but not the optional header,
    // whose presence the top flag records.
    let header = if flags & 0x8000_0000 != 0 { 32 } else { 0 };
    let start = data.len().saturating_sub(size as usize + header);
    &data[..start]
}

/// Appends signed little-endian integer samples of `width` bytes as floats.
fn push_signed(bytes: &[u8], width: usize, out: &mut Vec<f32>) {
    let scale = scale(width as u32 * 8);
    out.reserve(bytes.len() / width.max(1));
    for sample in bytes.chunks_exact(width) {
        let value = match width {
            1 => f32::from(sample[0] as i8),
            2 => f32::from(i16::from_le_bytes([sample[0], sample[1]])),
            3 => (i32::from_le_bytes([0, sample[0], sample[1], sample[2]]) >> 8) as f32,
            _ => i32::from_le_bytes([sample[0], sample[1], sample[2], sample[3]]) as f32,
        };
        out.push(value * scale);
    }
}

/// Appends unsigned 8-bit samples as floats, centred on zero.
fn push_unsigned_8(bytes: &[u8], out: &mut Vec<f32>) {
    out.extend(bytes.iter().map(|byte| (f32::from(*byte) - 128.0) / 128.0));
}

/// Appends 32-bit little-endian float samples unchanged.
fn push_float(bytes: &[u8], out: &mut Vec<f32>) {
    out.reserve(bytes.len() / 4);
    for sample in bytes.as_chunks::<4>().0 {
        out.push(f32::from_le_bytes(*sample));
    }
}

/// The sample rate from the file header a Shorten stream carried verbatim.
///
/// The stream quotes its source's RIFF, AIFF or AU header, so the rate is
/// read out of whichever one it is.
fn envelope_rate(bytes: &[u8]) -> Option<u32> {
    match bytes.get(..4)? {
        b"RIFF" if bytes.get(8..12)? == b"WAVE" => riff_rate(bytes),
        b"FORM" if matches!(bytes.get(8..12)?, b"AIFF" | b"AIFC") => aiff_rate(bytes),
        b".snd" => Some(u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?)),
        _ => None,
    }
}

/// The rate in a RIFF `fmt ` chunk.
fn riff_rate(bytes: &[u8]) -> Option<u32> {
    let mut offset = 12_usize;
    while let Some(id) = bytes.get(offset..offset + 4) {
        let size = u32::from_le_bytes(bytes.get(offset + 4..offset + 8)?.try_into().ok()?) as usize;
        if id == b"fmt " {
            let rate = bytes.get(offset + 12..offset + 16)?;
            return Some(u32::from_le_bytes(rate.try_into().ok()?));
        }
        offset = offset.checked_add(8)?.checked_add(size)?;
        offset += size & 1;
    }
    None
}

/// The rate in an AIFF `COMM` chunk.
fn aiff_rate(bytes: &[u8]) -> Option<u32> {
    let mut offset = 12_usize;
    while let Some(id) = bytes.get(offset..offset + 4) {
        let size = u32::from_be_bytes(bytes.get(offset + 4..offset + 8)?.try_into().ok()?) as usize;
        if id == b"COMM" {
            let rate = extended(bytes.get(offset + 16..offset + 26)?)?;
            return Some(rate.round() as u32);
        }
        offset = offset.checked_add(8)?.checked_add(size)?;
        offset += size & 1;
    }
    None
}

/// Reads an 80-bit IEEE extended float, the form an AIFF header stores.
fn extended(bytes: &[u8]) -> Option<f64> {
    let bytes: [u8; 10] = bytes.get(..10)?.try_into().ok()?;
    let negative = bytes[0] & 0x80 != 0;
    let exponent = i32::from(u16::from(bytes[0] & 0x7f) << 8 | u16::from(bytes[1]));
    let mantissa = u64::from_be_bytes(bytes[2..10].try_into().ok()?);
    if exponent == 0 && mantissa == 0 {
        return Some(0.0);
    }
    // The mantissa's top bit is the integer part, which is explicit in this
    // encoding rather than implied as in IEEE binary32 and binary64.
    let value = (mantissa as f64) * 2_f64.powi(exponent - 16_383 - 63);
    Some(if negative { -value } else { value })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decoded_file_reports_its_length_and_seeks() {
        let mut file = Pcm {
            samples: vec![0.0; 44_100 * 2],
            channels: 2,
            rate: 44_100,
            position: 0,
        };
        assert_eq!(file.frames(), 44_100);
        assert_eq!(file.total_duration(), Some(Duration::from_secs(1)));

        file.try_seek(Duration::from_millis(500)).expect("seeks");
        assert_eq!(file.position, 22_050 * 2);

        file.try_seek(Duration::from_secs(10)).expect("clamps");
        assert_eq!(file.position, 44_100 * 2);
        assert_eq!(file.next(), None, "seeking past the end stops the source");
    }

    #[test]
    fn signed_samples_are_scaled_to_full_scale() {
        let mut samples = Vec::new();
        push_signed(&[0x00, 0x00, 0xff, 0x7f, 0x00, 0x80], 2, &mut samples);
        assert_eq!(samples[0], 0.0);
        assert!((samples[1] - 0.9999).abs() < 0.001);
        assert_eq!(samples[2], -1.0);
    }

    #[test]
    fn twenty_four_bit_samples_are_scaled_to_full_scale() {
        let mut samples = Vec::new();
        push_signed(&[0x00, 0x00, 0x00, 0xff, 0xff, 0x7f], 3, &mut samples);
        assert_eq!(samples[0], 0.0);
        assert!((samples[1] - 0.9999).abs() < 0.001);
    }

    #[test]
    fn a_trailing_ape_tag_is_dropped() {
        let mut data = b"wvpk audio".to_vec();
        let plain = data.clone();
        // A header and footer with one 8-byte item between them.
        let mut tag = b"APETAGEX".to_vec();
        tag.extend_from_slice(&2000_u32.to_le_bytes());
        tag.extend_from_slice(&(32 + 8_u32).to_le_bytes());
        tag.extend_from_slice(&1_u32.to_le_bytes());
        tag.extend_from_slice(&0xa000_0000_u32.to_le_bytes());
        tag.extend_from_slice(&[0; 8]);
        let mut footer = tag.clone();
        footer[20..24].copy_from_slice(&0x8000_0000_u32.to_le_bytes());
        data.extend_from_slice(&tag);
        data.extend_from_slice(&[0; 8]);
        data.extend_from_slice(&footer);
        assert_eq!(without_ape_tag(&data), &plain[..]);
        assert_eq!(without_ape_tag(&plain), &plain[..], "untagged data is kept");
    }

    #[test]
    fn a_wav_envelope_gives_its_sample_rate() {
        let mut header = Vec::new();
        header.extend_from_slice(b"RIFF");
        header.extend_from_slice(&0_u32.to_le_bytes());
        header.extend_from_slice(b"WAVEfmt ");
        header.extend_from_slice(&16_u32.to_le_bytes());
        header.extend_from_slice(&1_u16.to_le_bytes());
        header.extend_from_slice(&2_u16.to_le_bytes());
        header.extend_from_slice(&44_100_u32.to_le_bytes());
        header.extend_from_slice(&[0; 8]);
        assert_eq!(envelope_rate(&header), Some(44_100));
    }

    #[test]
    fn an_aiff_envelope_gives_its_sample_rate() {
        let mut header = Vec::new();
        header.extend_from_slice(b"FORM");
        header.extend_from_slice(&0_u32.to_be_bytes());
        header.extend_from_slice(b"AIFFCOMM");
        header.extend_from_slice(&18_u32.to_be_bytes());
        header.extend_from_slice(&2_u16.to_be_bytes());
        header.extend_from_slice(&0_u32.to_be_bytes());
        header.extend_from_slice(&16_u16.to_be_bytes());
        // 44100 as an 80-bit extended float: 0x400E AC44 0000 0000 0000.
        header.extend_from_slice(&[0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0]);
        assert_eq!(envelope_rate(&header), Some(44_100));
    }

    #[test]
    fn a_missing_file_is_an_error() {
        let missing = std::env::temp_dir().join("blue-dove-no-such-file.ape");
        assert!(open(&missing).is_err());
    }
}
