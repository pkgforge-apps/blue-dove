//! The ABX listening test: ten blind trials between two tracks.
//!
//! Both tracks are known to the listener as A and B. Each trial picks one of
//! them as X without saying which, and the listener says which of the two X
//! is. The answer is scored, and a test runs ten trials.
//!
//! A listener who cannot tell the tracks apart still guesses right half the
//! time, so the count of right answers follows a binomial distribution: ten
//! trials, a chance of one half each. Nine right answers or more come up by
//! guessing 11 times in 1024, about 1.1%, which is under the 5% a test is
//! usually held to; eight (5.5%) is not, which is why nine is the count a
//! test has to meet.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::library::Track;

/// Trials a test runs, every one of them scored.
pub const TRIALS: usize = 10;

/// Right answers a listener needs to show they can tell the tracks apart.
///
/// At ten trials this is the smallest count under the usual 5% level: nine
/// or more right answers are guessed 11 times in 1024 (1.1%), eight or more
/// 56 times in 1024 (5.5%).
pub const PASSING: usize = 9;

/// One of the two tracks under test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// The track labelled A.
    A,
    /// The track labelled B.
    B,
}

impl Side {
    /// The letter the window shows for this side.
    pub fn label(self) -> &'static str {
        match self {
            Self::A => "A",
            Self::B => "B",
        }
    }
}

/// One of the three things the player can be offering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// The track labelled A.
    A,
    /// The track labelled B.
    B,
    /// The hidden track under test.
    X,
}

/// A test in progress.
pub struct Abx {
    /// The track labelled A.
    a: Track,
    /// The track labelled B.
    b: Track,
    /// Which of the two X is on the trial under way.
    x: Side,
    /// Whether each finished trial was answered right.
    results: Vec<bool>,
    /// The answer given for the trial under way, once there is one.
    answered: Option<Side>,
    /// What the player was last sent to, when it is offering anything.
    playing: Option<Choice>,
    /// State of the generator that picks X, so a test is not repeatable.
    seed: u64,
    /// What the log records, in the order it happened.
    entries: Vec<Entry>,
    /// When the test started, which the log's times are counted from.
    clock: Instant,
}

/// One line of the test's log.
struct Entry {
    /// How long after the test started it happened.
    at: Duration,
    /// What happened.
    what: EntryKind,
}

/// What a line of the log records.
enum EntryKind {
    /// The test started.
    Started,
    /// The test was started over, with a fresh draw of the sides.
    Restarted,
    /// A trial was answered, with the score as it stood after it.
    Answered {
        /// Right answers so far.
        correct: usize,
        /// Trials answered so far.
        answered: usize,
    },
    /// The last trial was answered, so the test is over.
    Finished,
}

impl EntryKind {
    /// The line this entry writes.
    fn text(&self) -> String {
        match self {
            Self::Started => "Test started.".to_string(),
            Self::Restarted => "Test restarted.".to_string(),
            Self::Answered { correct, answered } => format!("{correct:02}/{answered:02}"),
            Self::Finished => "Test finished.".to_string(),
        }
    }
}

impl Abx {
    /// Starts a test between `a` and `b`, with X drawn from `seed`.
    ///
    /// `seed` has to differ between runs for X to be unpredictable; the app
    /// draws it from its own generator.
    pub fn new(a: Track, b: Track, seed: u64) -> Self {
        let mut test = Self {
            a,
            b,
            // Replaced by the draw below, which needs the rest of the state.
            x: Side::A,
            results: Vec::new(),
            answered: None,
            playing: None,
            // A zero seed would leave the generator stuck, so a caller that
            // hands one over still gets a usable test.
            seed: seed.max(1),
            entries: vec![Entry {
                at: Duration::ZERO,
                what: EntryKind::Started,
            }],
            clock: Instant::now(),
        };
        test.x = test.draw();
        test
    }

    /// The track on `side`.
    pub fn track(&self, side: Side) -> &Track {
        match side {
            Side::A => &self.a,
            Side::B => &self.b,
        }
    }

    /// The track `choice` names, with X standing for the side drawn now.
    pub fn choice(&self, choice: Choice) -> &Track {
        match choice {
            Choice::A => &self.a,
            Choice::B => &self.b,
            Choice::X => self.track(self.x),
        }
    }

    /// The file of `choice`, which is what the player opens.
    pub fn path(&self, choice: Choice) -> PathBuf {
        self.choice(choice).path.clone()
    }

    /// The number of the trial under way, counting from one.
    ///
    /// Once the last trial is answered this stays at [`TRIALS`], since there
    /// is no next one to name.
    pub fn trial(&self) -> usize {
        let next = self.results.len() + usize::from(self.answered.is_none());
        next.clamp(1, TRIALS)
    }

    /// Trials finished so far.
    pub fn finished(&self) -> usize {
        self.results.len()
    }

    /// The answer to the trial under way, while it is being shown.
    pub fn answered(&self) -> Option<Side> {
        self.answered
    }

    /// Scores the trial under way against `guess`.
    ///
    /// Answers whether the guess was right, or `None` when the trial is
    /// already answered or the test is over: an answer that is taken twice
    /// would count twice.
    pub fn answer(&mut self, guess: Side) -> Option<bool> {
        if self.done() || self.answered.is_some() {
            return None;
        }
        let right = guess == self.x;
        self.results.push(right);
        self.answered = Some(guess);
        let correct = self.correct();
        let answered = self.results.len();
        self.entries.push(Entry {
            at: self.clock.elapsed(),
            what: EntryKind::Answered { correct, answered },
        });
        if self.done() {
            self.entries.push(Entry {
                at: self.clock.elapsed(),
                what: EntryKind::Finished,
            });
        }
        Some(right)
    }

    /// Moves to the trial after the one answered, drawing a new X.
    pub fn next(&mut self) {
        if self.answered.is_none() || self.done() {
            return;
        }
        self.answered = None;
        self.x = self.draw();
    }

    /// Whether every trial has been answered.
    pub fn done(&self) -> bool {
        self.results.len() >= TRIALS
    }

    /// Trials answered right.
    pub fn correct(&self) -> usize {
        self.results.iter().filter(|&&right| right).count()
    }

    /// Trials answered wrong.
    pub fn wrong(&self) -> usize {
        self.results.len() - self.correct()
    }

    /// Whether the listener showed they can tell the tracks apart.
    pub fn passed(&self) -> bool {
        self.correct() >= PASSING
    }

    /// The chance that guessing would have scored this well or better.
    ///
    /// This is the upper tail of a binomial with [`TRIALS`] trials and a
    /// chance of one half, for the right answers so far: the p-value an ABX
    /// test reports.
    pub fn p_value(&self) -> f64 {
        let ways: u32 = (self.correct()..=TRIALS)
            .map(|k| combinations(TRIALS, k))
            .sum();
        f64::from(ways) / (1_u32 << TRIALS) as f64
    }

    /// Starts the test again with the same two tracks, from a new `seed`.
    pub fn restart(&mut self, seed: u64) {
        self.entries.push(Entry {
            at: self.clock.elapsed(),
            what: EntryKind::Restarted,
        });
        self.results.clear();
        self.answered = None;
        self.playing = None;
        self.seed = seed.max(1);
        self.x = self.draw();
    }

    /// What the player was last sent to, when it is offering anything.
    pub fn playing(&self) -> Option<Choice> {
        self.playing
    }

    /// Records that the player was sent to `playing`.
    pub fn set_playing(&mut self, playing: Option<Choice>) {
        self.playing = playing;
    }

    /// The log of the test, as the window shows it and the signature covers.
    ///
    /// It names both files with their sums, so the record is of two files and
    /// not of two titles, and lists every trial with the score as it stood.
    /// The shape is the one foobar2000's comparator writes, and the sums are
    /// what a reader needs to tell that the two files really were different.
    pub fn log(&self, sums: Option<(&str, &str)>) -> String {
        let mut text = String::new();
        for side in [Side::A, Side::B] {
            let track = self.track(side);
            let name = track.path.file_name().map_or_else(
                || track.path.display().to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            text.push_str(&format!("File {}: {name}\n", side.label()));
            let sum = match (side, sums) {
                (Side::A, Some((a, _))) => a,
                (Side::B, Some((_, b))) => b,
                _ => "(not read)",
            };
            text.push_str(&format!("SHA256: {sum}\n"));
        }
        text.push('\n');
        for entry in &self.entries {
            text.push_str(&format!("{} : {}\n", clock(entry.at), entry.what.text()));
        }
        if self.done() {
            let chance = self.p_value() * 100.0;
            text.push_str(&format!(
                "\nTotal: {}/{}\np-value: {:.4} ({chance:.2}%)\n",
                self.correct(),
                TRIALS,
                self.p_value(),
            ));
        }
        text
    }

    /// The signature over the log, which is what a reader recomputes.
    ///
    /// It is the SHA-256 of [`log`](Self::log) as it stands once the test is
    /// over: take the log without its signature block, hash it, and the two
    /// have to match. That binds the result to the two files, since their
    /// sums are the first lines of what was signed.
    pub fn signature(&self, sums: Option<(&str, &str)>) -> Option<String> {
        self.done()
            .then(|| crate::digest::hex(self.log(sums).as_bytes()))
    }

    /// The log with the signature block under it, as Copy Log hands it out.
    ///
    /// Everything above the `-- signature --` line is exactly what was
    /// signed, so a reader can hash the top of this text and compare it with
    /// the line under the marker.
    pub fn signed_log(&self, sums: Option<(&str, &str)>) -> String {
        let mut text = self.log(sums);
        if let Some(signature) = self.signature(sums) {
            text.push_str("-- signature --\n");
            text.push_str(&signature);
            text.push('\n');
        }
        text
    }

    /// Draws the side X takes on the next trial.
    ///
    /// The generator is the plain xorshift the shuffle uses, so no
    /// dependency is needed for two outcomes.
    fn draw(&mut self) -> Side {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        if self.seed & 1 == 0 { Side::A } else { Side::B }
    }
}

impl From<Side> for Choice {
    /// The choice that plays the track on `side`.
    fn from(side: Side) -> Self {
        match side {
            Side::A => Self::A,
            Side::B => Self::B,
        }
    }
}

/// A moment as `hh:mm:ss`, counted from the start of the test.
fn clock(at: Duration) -> String {
    let seconds = at.as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    )
}

/// How many ways `k` of `n` can be chosen, which the binomial needs.
///
/// `n` is ten here, so the result fits a `u32` many times over.
fn combinations(n: usize, k: usize) -> u32 {
    if k > n {
        return 0;
    }
    let k = k.min(n - k);
    (1..=k).fold(1_u32, |ways, i| ways * (n - k + i) as u32 / i as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A track that only needs a name, since a test never opens one.
    fn track(title: &str) -> Track {
        Track {
            path: PathBuf::from(format!("/music/{title}.flac")),
            title: title.to_string(),
            artist: "Someone".to_string(),
            album: "An Album".to_string(),
            album_artist: "Someone".to_string(),
            year: Some(2007),
            disc: None,
            number: Some(1),
            duration: Duration::from_secs(200),
            cover: None,
            directory: PathBuf::from("/music"),
            tagged_album: true,
        }
    }

    fn test(seed: u64) -> Abx {
        Abx::new(track("a"), track("b"), seed)
    }

    /// The side `side` is not on.
    fn other(side: Side) -> Side {
        match side {
            Side::A => Side::B,
            Side::B => Side::A,
        }
    }

    /// Answers `trials` trials, right until `right` are used up.
    fn score(test: &mut Abx, right: usize) {
        for index in 0..TRIALS {
            let x = test.x;
            let guess = if index < right { x } else { other(x) };
            test.answer(guess).expect("the trial was unanswered");
            test.next();
        }
    }

    #[test]
    fn a_test_runs_ten_trials_and_ends() {
        let mut test = test(1);
        assert_eq!(test.trial(), 1);
        for trial in 1..=TRIALS {
            assert_eq!(test.trial(), trial);
            test.answer(Side::A);
            assert!(!test.done() || trial == TRIALS);
            test.next();
        }
        assert!(test.done());
        assert_eq!(test.finished(), TRIALS);
        assert_eq!(test.correct() + test.wrong(), TRIALS);
    }

    #[test]
    fn nine_of_ten_answers_passes_and_eight_does_not() {
        let mut passing = test(1);
        score(&mut passing, 9);
        assert!(passing.passed());

        let mut failing = test(1);
        score(&mut failing, 8);
        assert!(!failing.passed());
    }

    #[test]
    fn a_score_is_judged_by_how_likely_guessing_would_reach_it() {
        // Nine or ten right answers out of ten are guessed eleven times in
        // 1024; eight or more, 56 times in 1024. The 5% level sits between.
        let mut test = test(1);
        score(&mut test, 9);
        assert_eq!(test.p_value(), 11.0 / 1024.0);
        assert!(test.p_value() < 0.05);

        let mut test = self::test(2);
        score(&mut test, 8);
        assert_eq!(test.p_value(), 56.0 / 1024.0);
        assert!(test.p_value() > 0.05);
    }

    #[test]
    fn a_trial_is_scored_once_and_only_while_it_is_open() {
        let mut test = test(3);
        let x = test.x;
        assert!(test.answered().is_none());
        assert!(test.answer(x).is_some());
        assert_eq!(test.answered(), Some(x));
        assert!(
            test.answer(x).is_none(),
            "a second answer would count twice"
        );
        assert_eq!(test.finished(), 1);
        assert_eq!(test.correct(), 1);
    }

    #[test]
    fn the_answer_is_kept_while_x_stays_where_it_was_drawn() {
        let mut test = test(4);
        let x = test.x;
        assert_eq!(test.answered(), None);
        test.answer(other(x));
        assert_eq!(test.answered(), Some(other(x)));
        assert_eq!(test.correct(), 0, "a wrong answer scores nothing");
        assert_eq!(test.x, x, "the answer does not move X");
    }

    #[test]
    fn a_new_trial_draws_x_again_and_forgets_the_answer() {
        let mut test = test(5);
        test.answer(Side::A);
        test.next();
        assert_eq!(test.answered(), None);
        assert_eq!(test.trial(), 2);
    }

    #[test]
    fn x_falls_on_both_sides_over_a_run_of_trials() {
        // Two outcomes drawn independently: over ten trials of a few seeds
        // both sides have to come up, or the draw is stuck on one.
        let mut sides = [false; 2];
        for seed in 1..=8 {
            let mut test = test(seed);
            for _ in 0..TRIALS {
                sides[usize::from(test.x == Side::B)] = true;
                test.answer(test.x);
                test.next();
            }
        }
        assert_eq!(sides, [true, true]);
    }

    #[test]
    fn restarting_clears_the_score_and_asks_a_new_question() {
        let mut test = test(6);
        score(&mut test, 10);
        assert!(test.done());
        test.restart(7);
        assert!(!test.done());
        assert_eq!(test.correct(), 0);
        assert_eq!(test.finished(), 0);
        assert_eq!(test.trial(), 1);
        assert_eq!(test.answered(), None);
    }

    #[test]
    fn a_player_switched_to_x_plays_the_side_x_stands_for() {
        let mut test = test(8);
        assert_eq!(test.path(Choice::X), test.path(test.x.into()));
        assert_eq!(test.path(Choice::A), PathBuf::from("/music/a.flac"));
        assert_eq!(test.path(Choice::B), PathBuf::from("/music/b.flac"));
        test.set_playing(Some(Choice::X));
        assert_eq!(test.playing(), Some(Choice::X));
    }

    #[test]
    fn the_log_names_both_files_with_their_sums() {
        let test = test(1);
        let log = test.log(Some(("aaaa", "bbbb")));
        assert!(log.contains("File A: a.flac"), "{log}");
        assert!(log.contains("SHA256: aaaa"), "{log}");
        assert!(log.contains("File B: b.flac"), "{log}");
        assert!(log.contains("SHA256: bbbb"), "{log}");
        assert!(
            test.log(None).contains("SHA256: (not read)"),
            "a sum that has not been read says so"
        );
    }

    #[test]
    fn the_log_counts_every_trial_and_totals_them() {
        let mut test = test(1);
        score(&mut test, 9);
        let log = test.log(Some(("a", "b")));
        assert!(log.contains("Test started."), "{log}");
        assert!(log.contains("09/10"), "the last trial's score is in it");
        assert!(log.contains("Test finished."), "{log}");
        assert!(log.contains("Total: 9/10"), "{log}");
        assert!(log.contains("p-value: 0.0107 (1.07%)"), "{log}");
        let fresh = self::test(2);
        assert!(
            !fresh.log(Some(("a", "b"))).contains("Total"),
            "a test under way has no total yet"
        );
    }

    #[test]
    fn a_restart_is_written_into_the_log_it_keeps() {
        let mut test = test(1);
        score(&mut test, 3);
        test.restart(2);
        let log = test.log(Some(("a", "b")));
        assert!(log.contains("Test restarted."), "{log}");
        assert_eq!(
            log.matches("Test started.").count(),
            1,
            "the run before the restart stays in the log"
        );
    }

    #[test]
    fn the_signature_is_the_hash_of_the_log_it_signs() {
        let mut test = test(1);
        assert!(
            test.signature(Some(("a", "b"))).is_none(),
            "a test under way signs nothing"
        );

        score(&mut test, 9);
        let log = test.log(Some(("a", "b")));
        let signature = test
            .signature(Some(("a", "b")))
            .expect("a finished test is signed");
        assert_eq!(signature, crate::digest::hex(log.as_bytes()));
        assert_eq!(signature.len(), 64, "it is a SHA-256, written out");
        assert_ne!(
            signature,
            test.signature(Some(("c", "b"))).expect("signed"),
            "the sums are signed, so a signature names its files"
        );
    }

    #[test]
    fn the_text_above_the_signature_line_hashes_to_the_signature() {
        // This is the whole point of the signature: everything above the
        // marker is the signed text, and hashing it gives the line under it.
        let mut test = test(1);
        score(&mut test, 9);
        let text = test.signed_log(Some(("a", "b")));
        let (body, block) = text
            .split_once("-- signature --\n")
            .expect("the block is under the log");
        assert_eq!(crate::digest::hex(body.as_bytes()), block.trim_end());
    }

    #[test]
    fn the_combinations_the_p_value_is_built_from_are_exact() {
        assert_eq!(combinations(10, 0), 1);
        assert_eq!(combinations(10, 5), 252);
        assert_eq!(combinations(10, 10), 1);
        assert_eq!(combinations(4, 5), 0);
    }
}
