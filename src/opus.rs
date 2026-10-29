//! An Ogg Opus decoder, for the files symphonia cannot open.
//!
//! rodio decodes through symphonia, which has no Opus decoder, so an `.opus`
//! file would fail with "the format of the data has not been recognized".
//! This reads the Ogg container itself and decodes the packets with
//! `opus-rs`, a pure Rust decoder, then hands the samples to rodio as a
//! [`Source`].
//!
//! Packets are decoded as the audio thread asks for them, one packet at a
//! time. Decoding a whole file takes about a second on a long track, and
//! doing that before playback is what made a track wait before it started.
//! The length comes from the last page's granule position and a seek from the
//! reader's page index, so neither has to decode the file either.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

use ogg::PacketReader;
use opus_rs::OpusDecoder;
use rodio::source::{SeekError, Source};
use rodio::{ChannelCount, SampleRate};

use crate::error::At;

/// Opus always decodes at 48 kHz, whatever rate it was encoded at.
const OPUS_RATE: u32 = 48_000;

/// Samples decoded before a seek target, so the decoder has settled by the
/// time it reaches it.
const PRE_ROLL: u64 = (OPUS_RATE as u64 * 80) / 1000;

/// An Ogg Opus stream, decoded as it plays.
///
/// `channels` is the stream's channel count and `frame` holds the packet
/// being played, so the file is read one packet ahead of the sound rather
/// than whole.
pub struct OpusFile {
    /// The Ogg reader, left where the last packet ended.
    reader: PacketReader<BufReader<File>>,
    /// The decoder, which carries state from packet to packet.
    decoder: OpusDecoder,
    /// Channels in the stream.
    channels: u16,
    /// The stream's serial number, which a seek filters pages by.
    serial: u32,
    /// Samples of the current packet, interleaved, not yet handed out.
    frame: Vec<f32>,
    /// How many of `frame` have been handed out.
    offset: usize,
    /// Interleaved samples still to drop, from the pre-skip or from a seek.
    discard: usize,
    /// Where one packet is decoded, so the buffer is reused.
    packet: Vec<f32>,
    /// Stream samples the file holds, pre-skip included.
    total: u64,
    /// Samples the encoder asks the player to drop at the start.
    pre_skip: u64,
    /// Whether the stream has run out, so a seek past the end stays silent.
    ended: bool,
}

impl OpusFile {
    /// Opens `path`, or explains why it could not.
    pub fn open(path: &Path) -> Result<Self, String> {
        let total = last_granule(path)?;
        let file = File::open(path).at(path)?;
        let mut reader = PacketReader::new(BufReader::new(file));

        let head = reader
            .read_packet()
            .at(path)?
            .ok_or_else(|| format!("{}: not an Ogg Opus file", path.display()))?;
        let (channels, pre_skip) = head_info(&head.data, path)?;
        let decoder = OpusDecoder::new(OPUS_RATE as i32, usize::from(channels)).at(path)?;

        let mut file = Self {
            reader,
            decoder,
            channels,
            serial: head.stream_serial(),
            frame: Vec::new(),
            offset: 0,
            discard: pre_skip as usize * usize::from(channels),
            packet: Vec::new(),
            total,
            pre_skip,
            ended: false,
        };
        file.prime();
        if file.frame.is_empty() {
            return Err(format!("{}: no audio in the file", path.display()));
        }
        Ok(file)
    }

    /// Decodes packets until the current one holds samples, or the file ends.
    ///
    /// `discard` is applied to whatever is in hand first, since a seek fills
    /// `frame` from its own landing page before any new packet is decoded.
    fn prime(&mut self) {
        loop {
            if self.discard > 0 {
                let drop = self
                    .discard
                    .min(self.frame.len().saturating_sub(self.offset));
                self.offset += drop;
                self.discard -= drop;
            }
            if self.offset < self.frame.len() {
                return;
            }
            self.offset = 0;
            self.frame.clear();
            if self.ended || !self.decode_packet() {
                self.ended = true;
                return;
            }
        }
    }

    /// Decodes the next packet of audio into `frame`.
    ///
    /// Answers whether there was one: the tags are skipped and the end of the
    /// file, or a packet that will not decode, ends the track.
    fn decode_packet(&mut self) -> bool {
        let channels = usize::from(self.channels);
        loop {
            let packet = match self.reader.read_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) | Err(_) => return false,
            };
            if !audio_packet(&packet.data) {
                continue;
            }
            let Some(count) = self.decode_into(&packet.data) else {
                return false;
            };
            self.frame
                .extend_from_slice(&self.packet[..count * channels]);
            if !self.frame.is_empty() {
                return true;
            }
        }
    }

    /// Decodes one packet into `packet`, answering its samples per channel.
    fn decode_into(&mut self, data: &[u8]) -> Option<usize> {
        let channels = usize::from(self.channels);
        let frames = frame_samples(data[0]);
        self.packet.clear();
        self.packet.resize(frames * channels, 0.0);
        self.decoder.decode(data, frames, &mut self.packet).ok()
    }

    /// Learns where the reader landed, and drops the samples before `target`.
    ///
    /// A seek lands at the start of a page, whose granule position is the
    /// end of its last packet. Decoding the whole page gives the samples it
    /// holds, and the page therefore starts at that granule minus them, which
    /// is the position to count the dropped samples from.
    fn anchor(&mut self, target: u64) {
        let channels = usize::from(self.channels);
        let mut end = None;
        let mut page = 0usize;
        while let Ok(Some(packet)) = self.reader.read_packet() {
            if !audio_packet(&packet.data) {
                continue;
            }
            let granule = packet.absgp_page();
            let Some(count) = self.decode_into(&packet.data) else {
                break;
            };
            match end {
                // A page that ends with an unfinished packet records no
                // granule, so its samples cannot be counted from anything.
                None if granule == u64::MAX => continue,
                None => {
                    end = Some(granule);
                    page = count;
                }
                Some(page_end) if granule != page_end => {
                    // The first packet of the next page starts exactly where
                    // the landed page ended, so the landing is known now.
                    self.frame
                        .extend_from_slice(&self.packet[..count * channels]);
                    break;
                }
                Some(_) => page += count,
            }
            self.frame
                .extend_from_slice(&self.packet[..count * channels]);
        }
        if let Some(end) = end {
            let start = end.saturating_sub(page as u64);
            self.discard = target.saturating_sub(start) as usize * channels;
        }
    }

    /// Frames in the stream, for the duration.
    fn frames(&self) -> u64 {
        self.total.saturating_sub(self.pre_skip)
    }
}

impl Iterator for OpusFile {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        self.prime();
        let sample = self.frame.get(self.offset).copied();
        if sample.is_some() {
            self.offset += 1;
        }
        sample
    }
}

impl Source for OpusFile {
    fn current_span_len(&self) -> Option<usize> {
        Some(self.frame.len())
    }

    fn channels(&self) -> ChannelCount {
        ChannelCount::new(self.channels).expect("Opus has one or two channels")
    }

    fn sample_rate(&self) -> SampleRate {
        SampleRate::new(OPUS_RATE).expect("Opus always decodes at 48 kHz")
    }

    fn total_duration(&self) -> Option<Duration> {
        Some(Duration::from_secs_f64(
            self.frames() as f64 / f64::from(OPUS_RATE),
        ))
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        let target = self.pre_skip + (position.as_secs_f64() * f64::from(OPUS_RATE)) as u64;
        self.frame.clear();
        self.offset = 0;
        self.discard = 0;
        if target >= self.total {
            self.ended = true;
            return Ok(());
        }

        // A backwards seek needs the decoder's state rebuilt from the packet
        // before the target, and the reader's index is what finds it.
        self.ended = false;
        self.decoder = OpusDecoder::new(OPUS_RATE as i32, usize::from(self.channels))
            .map_err(|error| SeekError::Other(std::sync::Arc::new(std::io::Error::other(error))))?;

        // Landing a little before the target gives the decoder the frames it
        // needs to settle, which the dropped samples then take back off.
        let goal = target.saturating_sub(PRE_ROLL);
        let found = self
            .reader
            .seek_absgp(Some(self.serial), goal)
            .map_err(|error| SeekError::Other(std::sync::Arc::new(error)))?;
        if !found {
            // The index could not place a target the file's own length says
            // is inside it, so its pages and its last granule disagree.
            // Walking from the start is slower but still lands right.
            self.reader
                .seek_bytes(SeekFrom::Start(0))
                .map_err(|error| SeekError::Other(std::sync::Arc::new(error)))?;
        }
        self.anchor(target);
        self.prime();
        Ok(())
    }
}

/// Whether a packet carries audio rather than the stream's headers.
fn audio_packet(data: &[u8]) -> bool {
    !data.is_empty() && !data.starts_with(b"OpusHead") && !data.starts_with(b"OpusTags")
}

/// Reads the `OpusHead` packet: the channel count and the samples to drop.
fn head_info(data: &[u8], path: &Path) -> Result<(u16, u64), String> {
    if !data.starts_with(b"OpusHead") || data.len() < 19 {
        return Err(format!("{}: not an Ogg Opus file", path.display()));
    }
    let channels = u16::from(data[9]);
    let pre_skip = u64::from(u16::from_le_bytes([data[10], data[11]]));
    Ok((channels, pre_skip))
}

/// Stream samples the file holds, pre-skip included, from its last page.
///
/// An Ogg page records the granule position of the last packet that ends in
/// it, and the last page records the length of the whole stream, so the
/// length costs a read of the file's tail rather than a decode of the file.
fn last_granule(path: &Path) -> Result<u64, String> {
    let mut file = File::open(path).at(path)?;
    let length = file.seek(SeekFrom::End(0)).at(path)?;
    let window = length.min(1 << 16);
    file.seek(SeekFrom::Start(length - window)).at(path)?;
    let mut tail = vec![0u8; window as usize];
    file.read_exact(&mut tail).at(path)?;

    if tail.len() >= 27 {
        for start in (0..=tail.len() - 27).rev() {
            if &tail[start..start + 4] != b"OggS" {
                continue;
            }
            let granule = i64::from_le_bytes(
                tail[start + 6..start + 14]
                    .try_into()
                    .expect("the slice is eight bytes"),
            );
            if granule > 0 {
                return Ok(granule as u64);
            }
        }
    }
    Err(format!("{}: no Ogg page with a length", path.display()))
}

/// Samples per channel in a packet, from the frame duration in its TOC byte.
///
/// The TOC carries a configuration number and a frame-count code; the
/// duration is 2.5, 5, 10, 20, 40 or 60 ms per frame, up to three frames.
fn frame_samples(toc: u8) -> usize {
    let config = usize::from(toc >> 3);
    // The configuration numbers group by mode, and each group of four
    // counts up in duration: SILK 10/20/40/60 ms, hybrid 10/20, CELT
    // 2.5/5/10/20 ms.
    let frame_ms = match config {
        0..=3 => [10.0, 20.0, 40.0, 60.0][config],
        4..=11 => [10.0, 20.0, 10.0, 20.0][config % 4],
        12..=15 => [10.0, 20.0][config % 2],
        16..=31 => [2.5, 5.0, 10.0, 20.0][config % 4],
        _ => 20.0,
    };
    let count = match toc & 0x03 {
        0 => 1,
        1 | 2 => 2,
        _ => 3,
    };
    let samples = (f64::from(OPUS_RATE) * frame_ms / 1000.0) as usize;
    samples * count
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogg::writing::{PacketWriteEndInfo, PacketWriter};
    use std::path::PathBuf;

    /// A CELT packet of 20 ms of silence, as libopus writes one for DTX.
    const SILENCE: [u8; 3] = [0xF8, 0xFF, 0xFE];

    /// Frames one [`SILENCE`] packet decodes to.
    const SILENCE_FRAMES: usize = 960;

    #[test]
    fn a_short_frame_is_a_twenty_millisecond_packet() {
        // Configuration 9 is CELT 20 ms, one frame.
        assert_eq!(frame_samples(9 << 3), 960);
    }

    #[test]
    fn a_long_frame_holds_three_twenty_millisecond_frames() {
        assert_eq!(frame_samples((9 << 3) | 0x03), 960 * 3);
    }

    #[test]
    fn a_silk_sixty_millisecond_frame_is_2880_samples() {
        assert_eq!(frame_samples(3 << 3), 2880);
    }

    #[test]
    fn a_missing_file_is_an_error() {
        let missing = std::env::temp_dir().join("blue-dove-no-such-file.opus");
        assert!(OpusFile::open(&missing).is_err());
    }

    /// A fresh file named `file_name` in a temporary directory of its own,
    /// with the writer its packets go through.
    fn scratch(name: &str, file_name: &str) -> (PathBuf, PathBuf, PacketWriter<'static, File>) {
        let directory = std::env::temp_dir().join(format!("blue-dove-opus-{name}"));
        std::fs::create_dir_all(&directory).expect("makes a temporary directory");
        let path = directory.join(file_name);
        let file = File::create(&path).expect("creates the file");
        (directory, path, PacketWriter::new(file))
    }

    /// Writes the `OpusHead` and `OpusTags` packets of a stream with
    /// `channels` channels, which every file opens with.
    fn write_headers(writer: &mut PacketWriter<'_, File>, channels: u8) {
        let mut head = Vec::from(*b"OpusHead");
        head.extend_from_slice(&[1, channels]);
        head.extend_from_slice(&0u16.to_le_bytes());
        head.extend_from_slice(&OPUS_RATE.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes());
        head.push(0);
        writer
            .write_packet(head, 1, PacketWriteEndInfo::EndPage, 0)
            .expect("writes the head");

        let mut tags = Vec::from(*b"OpusTags");
        tags.extend_from_slice(&0u32.to_le_bytes());
        tags.extend_from_slice(&0u32.to_le_bytes());
        writer
            .write_packet(tags, 1, PacketWriteEndInfo::EndPage, 0)
            .expect("writes the tags");
    }

    /// Writes an Ogg Opus file of `packets` silence packets, one page each
    /// `per_page`, into a directory of its own.
    fn opus_file(name: &str, packets: usize, per_page: usize) -> (PathBuf, PathBuf) {
        let (directory, path, mut writer) = scratch(name, "silence.opus");
        write_headers(&mut writer, 1);

        for index in 1..=packets {
            let end = if index == packets {
                PacketWriteEndInfo::EndStream
            } else if index % per_page == 0 {
                PacketWriteEndInfo::EndPage
            } else {
                PacketWriteEndInfo::NormalPacket
            };
            let granule = (index * SILENCE_FRAMES) as u64;
            writer
                .write_packet(&SILENCE[..], 1, end, granule)
                .expect("writes a packet");
        }
        (directory, path)
    }

    /// Writes an Ogg Opus file of a rising tone, `seconds` long, so a seek
    /// can be told apart from the audio before it.
    fn opus_tone(name: &str, seconds: usize) -> (PathBuf, PathBuf) {
        let (directory, path, mut writer) = scratch(name, "tone.opus");
        write_headers(&mut writer, 2);

        let mut encoder =
            opus_rs::OpusEncoder::new(OPUS_RATE as i32, 2, opus_rs::Application::Audio)
                .expect("builds a decoder");
        encoder.complexity = 3;
        let mut packet = vec![0u8; 4000];
        let frames = SILENCE_FRAMES;
        let packets = seconds * 50;
        for index in 1..=packets {
            let mut pcm = Vec::with_capacity(frames * 2);
            for frame in 0..frames {
                let time = ((index - 1) * frames + frame) as f32 / OPUS_RATE as f32;
                let sample = 0.5 * (core::f32::consts::TAU * (220.0 + 200.0 * time) * time).sin();
                pcm.push(sample);
                pcm.push(sample);
            }
            let len = encoder
                .encode(&pcm, frames, &mut packet)
                .expect("encodes a packet");
            let encoded = packet[..len].to_vec();
            let end = if index == packets {
                PacketWriteEndInfo::EndStream
            } else if index % 5 == 0 {
                PacketWriteEndInfo::EndPage
            } else {
                PacketWriteEndInfo::NormalPacket
            };
            let granule = (index * frames) as u64;
            writer
                .write_packet(std::borrow::Cow::from(encoded), 1, end, granule)
                .expect("writes a packet");
        }
        (directory, path)
    }

    #[test]
    fn a_seek_plays_the_audio_that_belongs_there() {
        let (directory, path) = opus_tone("tone", 4);
        let mut sequential = OpusFile::open(&path).expect("opens");
        let channels = usize::from(sequential.channels);
        let skip = 2 * 48_000 * channels;
        sequential.by_ref().take(skip).count();
        let reference: Vec<f32> = sequential.by_ref().take(48_000 * channels).collect();

        let mut seeked = OpusFile::open(&path).expect("opens");
        seeked.try_seek(Duration::from_secs(2)).expect("seeks");
        let after: Vec<f32> = seeked.by_ref().take(48_000 * channels).collect();

        assert_eq!(after.len(), reference.len(), "a second of audio either way");
        let difference: f64 = reference
            .iter()
            .zip(&after)
            .map(|(left, right)| f64::from((left - right).abs()))
            .sum::<f64>()
            / reference.len() as f64;
        assert!(
            difference < 0.01,
            "the seek landed on other audio: mean difference {difference}"
        );
        std::fs::remove_dir_all(directory).expect("removes the temporary directory");
    }

    #[test]
    fn a_file_reports_its_length_without_decoding_it() {
        let (directory, path) = opus_file("length", 100, 5);
        let file = OpusFile::open(&path).expect("opens");
        assert_eq!(
            file.total_duration(),
            Some(Duration::from_secs_f64(
                (100 * SILENCE_FRAMES) as f64 / f64::from(OPUS_RATE)
            ))
        );
        // One packet is in hand, not the whole two seconds of audio: the rest
        // is still in the file, which is what keeps a start quick.
        assert_eq!(file.current_span_len(), Some(SILENCE_FRAMES));
        std::fs::remove_dir_all(directory).expect("removes the temporary directory");
    }

    #[test]
    fn a_file_plays_its_packets_one_after_another() {
        let (directory, path) = opus_file("play", 100, 5);
        let mut file = OpusFile::open(&path).expect("opens");
        let samples = file.by_ref().count();
        assert_eq!(samples, 100 * SILENCE_FRAMES);
        std::fs::remove_dir_all(directory).expect("removes the temporary directory");
    }

    #[test]
    fn seeking_leaves_the_track_at_the_position_asked_for() {
        let (directory, path) = opus_file("seek", 100, 5);
        let mut file = OpusFile::open(&path).expect("opens");
        let total = 100 * SILENCE_FRAMES;

        file.try_seek(Duration::from_millis(500)).expect("seeks");
        assert_eq!(file.by_ref().count(), total - 24_000);

        let mut file = OpusFile::open(&path).expect("opens again");
        file.try_seek(Duration::from_millis(1500)).expect("seeks");
        assert_eq!(file.by_ref().count(), total - 72_000);

        let mut file = OpusFile::open(&path).expect("opens again");
        file.try_seek(Duration::from_secs(60)).expect("clamps");
        assert_eq!(file.by_ref().count(), 0);

        // Seeking back from the end must play again rather than stay ended.
        let mut file = OpusFile::open(&path).expect("opens again");
        file.try_seek(Duration::from_secs(60)).expect("clamps");
        file.try_seek(Duration::from_millis(500))
            .expect("seeks back");
        assert_eq!(file.by_ref().count(), total - 24_000);
        std::fs::remove_dir_all(directory).expect("removes the temporary directory");
    }
}
