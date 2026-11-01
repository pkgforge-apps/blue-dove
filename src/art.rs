//! Album art decoded off the drawing thread.
//!
//! A scan stores a cover's encoded bytes and keeps only the key it was stored
//! under, so listing a library of thousands of tracks decodes no picture. One
//! thread decodes them here instead, from a single queue worked in the order
//! the keys were asked for: the picture the window is about to draw is queued
//! ahead of the ones that follow it.
//!
//! `Art::prefetch` queues the whole library when one lands, so a cover is
//! decoded whether or not its row is ever on screen, and only a picture the
//! window asked for makes it repaint. A key is queued once and its answer is
//! remembered, so a picture is decoded to be drawn or behind the frames but
//! never twice.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

use crate::cover::Cover;

/// How many decoded pictures are kept.
///
/// A cover is a quarter of a megabyte decoded, so a library of thousands of
/// albums would hold hundreds of megabytes if every one stayed. Past this the
/// picture decoded longest ago goes, and a key drawn again is decoded again.
const KEPT: usize = 512;

/// The decoded pictures, and the thread that decodes the rest.
#[derive(Default)]
pub struct Art {
    /// Pictures by key.
    ///
    /// `None` is a key with no picture behind it, which is remembered so the
    /// key is not asked for again.
    decoded: HashMap<String, Option<Arc<Cover>>>,
    /// The keys of those pictures, oldest first, so one can make room.
    order: VecDeque<String>,
    /// Keys handed to the thread, so one is queued once.
    asked: HashSet<String>,
    /// The keys among those the window asked for, which it repaints for.
    drawn: HashSet<String>,
    /// The thread, started when the first key is asked for.
    worker: Option<Worker>,
}

/// The queues to the thread that decodes pictures.
struct Worker {
    /// Keys to decode, in the order they were asked for.
    wanted: Sender<String>,
    /// Pictures that came back.
    results: Receiver<(String, Option<Arc<Cover>>)>,
}

impl Art {
    /// The picture for `key`, when it has been decoded.
    ///
    /// The outer `None` is a picture not decoded yet; the inner one is a key
    /// with no picture behind it, which the window draws as a placeholder.
    pub fn decoded(&self, key: &str) -> Option<Option<Arc<Cover>>> {
        self.decoded.get(key).cloned()
    }

    /// Asks the thread for `key`, once.
    ///
    /// A key asked for this way is one the window is about to draw, so its
    /// arrival makes the window repaint; `prefetch` asks without that. A key
    /// already queued by `prefetch` still counts as drawn here, or the window
    /// would wait for a picture nothing repaints for.
    pub fn request(&mut self, key: &str) {
        if self.decoded.contains_key(key) {
            return;
        }
        self.drawn.insert(key.to_string());
        self.queue(key);
    }

    /// Asks the thread for every key in `keys`.
    ///
    /// This is what fills the pictures behind the visible rows: a key the
    /// window asks for later is already queued or done, and one it never shows
    /// is decoded anyway so a scroll finds it ready.
    pub fn prefetch(&mut self, keys: impl IntoIterator<Item = String>) {
        for key in keys {
            self.queue(&key);
        }
    }

    /// Hands `key` to the thread, once.
    fn queue(&mut self, key: &str) {
        if self.decoded.contains_key(key) || !self.asked.insert(key.to_string()) {
            return;
        }
        let worker = self.worker.get_or_insert_with(Worker::start);
        // A closed queue is a thread that has ended, which only happens as the
        // app shuts down; the key is then left undecoded rather than retried.
        let _ = worker.wanted.send(key.to_string());
    }

    /// Takes in every picture that has come back.
    ///
    /// Says whether one the window asked for arrived, which is the only thing
    /// worth a repaint: the pictures `prefetch` queued behind them are found
    /// by the frame that draws them.
    pub fn poll(&mut self) -> bool {
        let Some(worker) = &self.worker else {
            return false;
        };
        let mut drawn = false;
        while let Ok((key, cover)) = worker.results.try_recv() {
            drawn |= self.drawn.remove(&key);
            if let Entry::Vacant(slot) = self.decoded.entry(key.clone()) {
                slot.insert(cover);
                self.order.push_back(key);
            }
        }
        self.make_room();
        drawn
    }

    /// Whether a picture is still being decoded.
    pub fn pending(&self) -> bool {
        self.decoded.len() < self.asked.len()
    }

    /// Drops the pictures decoded longest ago once too many are kept.
    fn make_room(&mut self) {
        while self.decoded.len() > KEPT {
            let Some(key) = self.order.pop_front() else {
                return;
            };
            self.decoded.remove(&key);
            // The key may be asked for again, and this time it is worth
            // decoding: what dropped it was room, not a missing picture.
            self.asked.remove(&key);
            self.drawn.remove(&key);
        }
    }
}

impl Worker {
    /// Starts the thread, which ends when the app drops its end of the queue.
    fn start() -> Self {
        let (wanted, queue) = mpsc::channel::<String>();
        let (sender, results) = mpsc::channel();
        thread::spawn(move || {
            for key in queue {
                let cover = decode_now(&key).map(Arc::new);
                if sender.send((key, cover)).is_err() {
                    break;
                }
            }
        });
        Self { wanted, results }
    }
}

/// Decodes the picture stored under `key`, on the calling thread.
///
/// The media widget names a track's art by a file, so it needs the pixels at
/// once rather than a frame later, and a track is played once at a time.
pub fn decode_now(key: &str) -> Option<Cover> {
    crate::cache::read_cover(key).and_then(|bytes| crate::cover::decode(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Polls `art` until `key` has an answer, or the test gives up.
    fn settle(art: &mut Art, key: &str) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            art.poll();
            if art.decoded(key).is_some() || Instant::now() > deadline {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn nothing_is_decoded_before_it_is_asked_for() {
        let art = Art::default();
        assert!(art.decoded("0000000000000000-0").is_none());
        assert!(!art.pending());
    }

    #[test]
    fn a_key_with_no_picture_is_answered_once_and_never_asked_for_again() {
        let mut art = Art::default();
        let key = "0000000000000000-0";
        art.request(key);
        settle(&mut art, key);

        assert!(
            art.decoded(key).is_some_and(|cover| cover.is_none()),
            "a key whose bytes are gone answers with no picture"
        );
        art.request(key);
        assert!(!art.pending(), "the answer is kept, so it is not retried");
    }

    #[test]
    fn a_prefetched_key_is_answered_without_being_drawn() {
        let mut art = Art::default();
        let key = "0000000000000000-0";
        art.prefetch([key.to_string()]);
        assert!(art.pending(), "the thread has a picture to decode");
        settle(&mut art, key);

        assert!(
            art.decoded(key).is_some_and(|cover| cover.is_none()),
            "a picture nobody drew is decoded too"
        );
        assert!(!art.pending(), "and the thread is idle again");
    }

    #[test]
    fn a_key_queued_behind_the_frames_still_repaints_when_drawn() {
        // A prefetched key is queued without being waited for; once the
        // window draws it, its arrival is the one that redraws the frame.
        let mut art = Art::default();
        let key = "0000000000000000-0";
        art.prefetch([key.to_string()]);
        art.request(key);

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut repainted = false;
        while Instant::now() < deadline && !repainted {
            repainted = art.poll();
            if art.pending() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        assert!(repainted, "the picture the window drew asked for a frame");
    }

    #[test]
    fn only_a_picture_the_window_drew_asks_for_a_repaint() {
        let mut art = Art::default();
        art.prefetch(["0000000000000000-0".to_string()]);
        art.request("0000000000000001-0");

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut repainted = false;
        while Instant::now() < deadline && !repainted {
            repainted = art.poll();
            if art.pending() {
                std::thread::sleep(Duration::from_millis(5));
            }
        }

        assert!(repainted, "the picture the window waited for asked for one");
        assert!(
            !art.poll(),
            "the arrivals behind it were taken in without one"
        );
    }

    #[test]
    fn a_key_asked_for_twice_is_queued_once() {
        let mut art = Art::default();
        let key = "0000000000000000-0";
        art.prefetch([key.to_string(), key.to_string()]);
        art.request(key);
        settle(&mut art, key);

        assert!(!art.pending());
        assert_eq!(art.asked.len(), 1, "one key, one entry");
    }

    #[test]
    fn the_pictures_decoded_longest_ago_go_first() {
        let mut art = Art::default();
        for index in 0..=KEPT {
            let key = format!("{index:016x}-0");
            art.order.push_back(key.clone());
            art.decoded.insert(key.clone(), None);
            art.asked.insert(key);
        }

        art.make_room();

        assert_eq!(art.decoded.len(), KEPT, "only the excess goes");
        assert!(
            !art.decoded.contains_key("0000000000000000-0"),
            "the picture decoded first is the one dropped"
        );
        assert!(
            art.decoded.contains_key(&format!("{KEPT:016x}-0")),
            "the newest one stays"
        );
        assert!(
            !art.asked.contains("0000000000000000-0"),
            "a dropped key may be decoded again"
        );
    }
}
