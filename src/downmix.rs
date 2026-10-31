//! Folding a surround track down to stereo.
//!
//! rodio's mixer keeps the first two channels of a source and throws the rest
//! away, which drops the centre channel a 5.1 mix puts the voice in and the
//! surrounds it puts much of the drums in. The result keeps the front pair
//! and loses the rest of the mix, so a surround track sounds nothing like the
//! stereo one it was released beside.
//!
//! This folds every channel in at the gains the ITU-R BS.775 downmix uses:
//! the centre is split between the two sides at -3 dB, each surround joins
//! its own side at -3 dB, and the LFE is dropped, since it carries nothing a
//! pair of speakers can reproduce. The level is left as it is rather than
//! normalised down, so a surround track plays at the level of a stereo one.

use std::time::Duration;

use rodio::source::{SeekError, Source};
use rodio::{ChannelCount, SampleRate};

/// The gain a centre or a surround channel has when it is folded in.
///
/// -3 dB, which is half the power: a channel split between two speakers is
/// worth less in each than it is on its own.
const FOLD: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Folds `source` to stereo, or answers it untouched when it is not a
/// surround stream.
pub fn stereo(source: Box<dyn Source + Send>) -> Box<dyn Source + Send> {
    let channels = usize::from(source.channels().get());
    if channels <= 2 {
        return source;
    }
    Box::new(Downmix {
        gains: gains(channels),
        inner: source,
        pending: None,
    })
}

/// A source of more than two channels, folded to two.
struct Downmix {
    /// The source being folded.
    inner: Box<dyn Source + Send>,
    /// The gain each source channel has in the left and the right output.
    gains: Vec<[f32; 2]>,
    /// The right sample of the frame just built, until it is asked for.
    pending: Option<f32>,
}

impl Iterator for Downmix {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if let Some(right) = self.pending.take() {
            return Some(right);
        }
        let mut left = 0.0;
        let mut right = 0.0;
        for gain in &self.gains {
            let sample = self.inner.next()?;
            left += sample * gain[0];
            right += sample * gain[1];
        }
        self.pending = Some(right);
        Some(left)
    }
}

impl Source for Downmix {
    fn current_span_len(&self) -> Option<usize> {
        self.inner
            .current_span_len()
            .map(|samples| samples / self.gains.len() * 2)
    }

    fn channels(&self) -> ChannelCount {
        ChannelCount::new(2).expect("two is a channel count")
    }

    fn sample_rate(&self) -> SampleRate {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }

    fn try_seek(&mut self, position: Duration) -> Result<(), SeekError> {
        self.pending = None;
        self.inner.try_seek(position)
    }
}

/// The gain each channel of a stream has in the left and the right output.
///
/// The channel order is the one WAV and FLAC use when a stream carries no
/// layout of its own: `L R` for two, `L R C` for three, `L R BL BR` for four
/// (a quadraphonic mix, since that is what four channels mean there), and
/// from five up `L R C [LFE] ...` with the back pair, a back centre and the
/// side pair after it. A stream with more channels than that is folded by
/// the same rule, its extra channels alternating between the two sides.
///
/// rodio reports a channel count and no layout, so the map is picked by the
/// count alone: a decoder handing its channels over in another order, Vorbis
/// with its centre first among them, is folded by the wrong one.
fn gains(channels: usize) -> Vec<[f32; 2]> {
    /// The front left channel, which reaches the left speaker as it is.
    const LEFT: [f32; 2] = [1.0, 0.0];
    /// The front right channel, which reaches the right speaker as it is.
    const RIGHT: [f32; 2] = [0.0, 1.0];
    /// A centre channel, split between the two speakers.
    const CENTRE: [f32; 2] = [FOLD, FOLD];
    /// A channel with nothing a pair of speakers can play.
    const SILENT: [f32; 2] = [0.0, 0.0];
    /// A channel behind or beside the listener on the left.
    const BACK_LEFT: [f32; 2] = [FOLD, 0.0];
    /// A channel behind or beside the listener on the right.
    const BACK_RIGHT: [f32; 2] = [0.0, FOLD];

    match channels {
        3 => vec![LEFT, RIGHT, CENTRE],
        4 => vec![LEFT, RIGHT, BACK_LEFT, BACK_RIGHT],
        5 => vec![LEFT, RIGHT, CENTRE, BACK_LEFT, BACK_RIGHT],
        6 => vec![LEFT, RIGHT, CENTRE, SILENT, BACK_LEFT, BACK_RIGHT],
        7 => vec![LEFT, RIGHT, CENTRE, SILENT, CENTRE, BACK_LEFT, BACK_RIGHT],
        8 => vec![
            LEFT, RIGHT, CENTRE, SILENT, BACK_LEFT, BACK_RIGHT, BACK_LEFT, BACK_RIGHT,
        ],
        _ => (0..channels)
            .map(|channel| match channel {
                0 => LEFT,
                1 => RIGHT,
                2 => CENTRE,
                3 => SILENT,
                channel if channel % 2 == 0 => BACK_LEFT,
                _ => BACK_RIGHT,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use rodio::buffer::SamplesBuffer;

    use super::*;

    /// A source of `channels` channels holding one frame of `samples`.
    fn frame(channels: u16, samples: Vec<f32>) -> Box<dyn Source + Send> {
        Box::new(SamplesBuffer::new(
            ChannelCount::new(channels).expect("a channel count"),
            SampleRate::new(48_000).expect("a sample rate"),
            samples,
        ))
    }

    #[test]
    fn a_five_one_source_keeps_its_centre_and_its_surrounds() {
        // Front left, front right, centre, LFE, back left, back right.
        let mut folded = stereo(frame(6, vec![0.5, 0.25, 0.5, 0.9, 0.25, 0.125]));
        assert_eq!(folded.channels().get(), 2, "the output is stereo");
        let left = folded.next().expect("a left sample");
        let right = folded.next().expect("a right sample");

        assert!(
            (left - (0.5 + FOLD * 0.5 + FOLD * 0.25)).abs() < 1e-6,
            "the centre and the back left are folded into the left side"
        );
        assert!(
            (right - (0.25 + FOLD * 0.5 + FOLD * 0.125)).abs() < 1e-6,
            "the centre and the back right are folded into the right side"
        );
    }

    #[test]
    fn the_lfe_is_left_out_of_the_fold() {
        // The LFE is loud enough to swamp the mix if it were folded in, and
        // a pair of speakers cannot play it anyway.
        let mut quiet = stereo(frame(6, vec![0.1, 0.1, 0.0, 0.0, 0.0, 0.0]));
        let mut loud = stereo(frame(6, vec![0.1, 0.1, 0.0, 1.0, 0.0, 0.0]));
        assert_eq!(quiet.next(), loud.next(), "the LFE changes nothing");
        assert_eq!(quiet.next(), loud.next(), "on either side");
    }

    #[test]
    fn a_quadraphonic_source_folds_its_back_pair_in() {
        // Front left, front right, back left, back right.
        let mut folded = stereo(frame(4, vec![0.5, 0.25, 0.5, 0.25]));
        assert!(
            (folded.next().expect("a left sample") - (0.5 + FOLD * 0.5)).abs() < 1e-6,
            "the back left joins the left side"
        );
        assert!(
            (folded.next().expect("a right sample") - (0.25 + FOLD * 0.25)).abs() < 1e-6,
            "the back right joins the right side"
        );
    }

    #[test]
    fn a_stereo_source_is_left_alone() {
        let mut source = stereo(frame(2, vec![0.1, 0.2, 0.3, 0.4]));
        assert_eq!(source.channels().get(), 2);
        assert_eq!(source.next(), Some(0.1));
        assert_eq!(source.next(), Some(0.2));
        assert_eq!(source.next(), Some(0.3));
    }

    #[test]
    fn a_mono_source_is_left_alone() {
        let mut source = stereo(frame(1, vec![0.4, 0.5]));
        assert_eq!(source.channels().get(), 1, "the mixer still duplicates it");
        assert_eq!(source.next(), Some(0.4));
    }

    #[test]
    fn a_seek_reaches_the_source_under_the_fold() {
        // The seek bar has to work on a surround track too, so the position
        // goes down to the decoder rather than being swallowed here. The
        // front-left channel carries its frame number, so the sample that
        // comes back after the seek says where playback landed.
        let mut samples = vec![0.0; 48_000 * 6];
        for frame in 0..48_000 {
            samples[frame * 6] = frame as f32;
        }
        let mut folded = stereo(frame(6, samples));
        folded
            .try_seek(Duration::from_millis(500))
            .expect("the seek lands");
        assert_eq!(
            folded.next(),
            Some(24_000.0),
            "half a second of playback, folded"
        );
    }

    /// A folded track must not leave the track after it at the wrong pitch.
    ///
    /// rodio's mixer converts whatever the player's queue reports at the
    /// moment it starts a span, and only looks again when that span ends. A
    /// fold that reported no span was converted to stereo once and never
    /// checked again, so a 96 kHz track left the mixer resampling at 96 kHz
    /// for the track after it, which played at the wrong speed.
    #[test]
    fn a_folded_track_leaves_the_next_one_at_its_own_pitch() {
        fn drain(out: &mut rodio::mixer::MixerSource) -> usize {
            // Pull until the queue has been silent for a quarter of a second,
            // then answer the last frame that carried a sample.
            let mut samples = Vec::new();
            for _ in 0..2_000_000u64 {
                samples.push(out.next().unwrap_or(0.0));
                let tail = &samples[samples.len().saturating_sub(24_000)..];
                if samples.len() > 100_000 && tail.iter().all(|value| *value == 0.0) {
                    break;
                }
            }
            samples
                .iter()
                .rposition(|sample| *sample != 0.0)
                .unwrap_or(0)
                / 2
        }

        // One second of a tone, folded and unfolded, at the app's own rates.
        let folded = stereo(Box::new(SamplesBuffer::new(
            ChannelCount::new(6).expect("six channels"),
            SampleRate::new(96_000).expect("a sample rate"),
            core::iter::repeat_n([0.5f32, 0.0, 0.0, 0.0, 0.0, 0.0], 96_000)
                .flatten()
                .collect::<Vec<f32>>(),
        )));
        let plain = SamplesBuffer::new(
            ChannelCount::new(2).expect("two channels"),
            SampleRate::new(44_100).expect("a sample rate"),
            vec![0.5f32; 88_200],
        );

        let (mixer, mut out) = rodio::mixer::mixer(
            ChannelCount::new(2).expect("two channels"),
            SampleRate::new(48_000).expect("a sample rate"),
        );
        let player = rodio::Player::connect_new(&mixer);

        player.append(folded);
        assert!(
            (47_000..49_000).contains(&drain(&mut out)),
            "a second of a folded 96 kHz track lasts a second at 48 kHz"
        );

        player.append(plain);
        let frames = drain(&mut out);
        assert!(
            (47_000..49_000).contains(&frames),
            "the track after it keeps its own pitch, got {frames} frames for one second"
        );
    }
}
