//! Peak envelope of a track, for the waveform seek bar.
//!
//! Working an envelope out means decoding the whole track, which takes a
//! moment, so it happens on a thread of its own and the result arrives over a
//! channel. The envelope of a track never changes, so it is kept in the cache
//! directory: a track played before draws its bar at once, and the thread is
//! only used the first time. [`peaks`] is the blocking form, which the tests
//! use.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use rodio::Source;

/// Bars in the envelope. More than a wide window has pixels, so a resized
/// window never runs out.
pub const BUCKETS: usize = 1600;

/// Envelope peaks, one per bucket, scaled so the loudest is 1.0.
pub type Peaks = Vec<f32>;

/// Starts computing the envelope of `path`, or reads the stored one.
///
/// A stored envelope is sent before this returns, so the seek bar is drawn in
/// the same frame the track starts rather than a moment later. Only a track
/// that has not been played before pays for the decode.
pub fn spawn(path: PathBuf) -> Receiver<Option<Peaks>> {
    let (sender, receiver) = mpsc::channel();
    if let Some(stored) = crate::cache::read_waveform(&path)
        && stored.len() == BUCKETS
    {
        let _ = sender.send(Some(stored));
        return receiver;
    }
    thread::spawn(move || {
        let computed = peaks(&path, BUCKETS).ok();
        if let Some(computed) = &computed {
            crate::cache::save_waveform(&path, computed);
        }
        let _ = sender.send(computed);
    });
    receiver
}

/// Computes the envelope of `path`: the loudest sample in each of `buckets`
/// equal slices of the track.
///
/// Returns an error when the file cannot be decoded or its length is
/// unknown, since the slices are then undefined.
pub fn peaks(path: &Path, buckets: usize) -> Result<Peaks, String> {
    let source = crate::decode::open(path)?;
    let channels = usize::from(source.channels().get());
    let rate = f64::from(source.sample_rate().get());
    let duration = source
        .total_duration()
        .ok_or_else(|| format!("{}: unknown length", path.display()))?;
    Ok(envelope(source, channels, rate, duration, buckets))
}

/// The loudest sample in each of `buckets` equal slices of `source`.
fn envelope(
    source: impl Iterator<Item = f32>,
    channels: usize,
    rate: f64,
    duration: std::time::Duration,
    buckets: usize,
) -> Peaks {
    let frames = (duration.as_secs_f64() * rate).max(1.0);
    let mut peaks = vec![0.0_f32; buckets.max(1)];
    for (index, sample) in source.enumerate() {
        let frame = (index / channels) as f64;
        let bucket = (frame / frames * peaks.len() as f64) as usize;
        let bucket = bucket.min(peaks.len() - 1);
        peaks[bucket] = peaks[bucket].max(sample.abs());
    }

    let loudest = peaks.iter().copied().fold(0.0_f32, f32::max);
    if loudest > 0.0 {
        for peak in &mut peaks {
            *peak /= loudest;
        }
    }
    peaks
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Writes a mono 16-bit WAV whose first half is silent and second half is
    /// a full-scale square wave, and returns the file it wrote.
    fn test_wav(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("blue-dove-{name}-{}.wav", std::process::id()));
        let rate = 8000_u32;
        let samples = rate;
        let mut data = Vec::with_capacity(samples as usize * 2);
        for index in 0..samples {
            let value: i16 = if index < samples / 2 { 0 } else { i16::MAX / 2 };
            data.extend_from_slice(&value.to_le_bytes());
        }

        let mut wav = Vec::new();
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16_u32.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&1_u16.to_le_bytes());
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 2).to_le_bytes());
        wav.extend_from_slice(&2_u16.to_le_bytes());
        wav.extend_from_slice(&16_u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(data.len() as u32).to_le_bytes());
        wav.extend_from_slice(&data);

        std::fs::write(&path, wav).expect("the test file is written");
        path
    }

    #[test]
    fn finds_the_loud_half_of_a_track() {
        let path = test_wav("waveform");
        let peaks = peaks(&path, 10).expect("the test file decodes");
        std::fs::remove_file(&path).expect("the test file is removed");

        assert_eq!(peaks.len(), 10);
        assert!(peaks[..4].iter().all(|&peak| peak < 0.01));
        assert!(peaks[6..].iter().all(|&peak| peak > 0.99));
    }

    #[test]
    fn a_missing_file_is_an_error() {
        let missing = std::env::temp_dir().join("blue-dove-no-such-track.wav");
        assert!(peaks(&missing, 10).is_err());
    }
}
