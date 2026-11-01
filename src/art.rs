//! Album art decoded off the drawing thread.
//!
//! A scan stores a cover's encoded bytes and keeps only the key it was stored
//! under, so listing a library of thousands of tracks decodes no picture. The
//! thread has two queues: the pictures the drawing pass asks for are decoded
//! first, since the window is waiting on them, and every other picture the
//! library holds is handed over once by `App::prefetch_art` and decoded behind
//! them, so a cover is ready even when its row is never painted. Only the
//! drawn ones make the window repaint, so the background work costs no frames.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use crate::cover::Cover;

/// How many decoded pictures are kept.
///
/// A cover is a quarter of a megabyte decoded, so a library of thousands of
/// albums would hold hundreds of megabytes if every one stayed. Past this the
/// picture left alone longest goes, and a key drawn again is decoded again.
const KEPT: usize = 512;

/// How long the thread waits for a picture before looking at the queue the
/// drawing pass fills.
const PEEK: Duration = Duration::from_millis(20);

/// The decoded pictures, and the thread that decodes the rest.
#[derive(Default)]
pub struct Art {
    /// Pictures by key, each with the moment it was last used.
    decoded: HashMap<String, (Option<Arc<Cover>>, u64)>,
    /// Keys the thread has been given, so one is asked for once.
    requested: HashSet<String>,
    /// Counter behind those moments.
    tick: u64,
    /// The thread, started when the first picture is asked for.
    worker: Option<Worker>,
}

/// The channels to the thread that decodes pictures.
struct Worker {
    /// Keys the drawing pass asked for, which are decoded first.
    wanted: Sender<String>,
    /// Keys to decode once nothing is waiting to be drawn.
    background: Sender<String>,
    /// Pictures that came back.
    results: Receiver<(String, Option<Arc<Cover>>)>,
    /// Keys given out that have not come back.
    outstanding: usize,
    /// The keys among those the window asked for, which it repaints for.
    wanted_outstanding: HashSet<String>,
}

impl Art {
    /// The picture for `key`, when it has been decoded.
    ///
    /// The outer `None` is a picture not decoded yet; the inner one is a key
    /// with no picture behind it, which the window draws as a placeholder and
    /// never asks for again.
    pub fn decoded(&mut self, key: &str) -> Option<Option<Arc<Cover>>> {
        self.tick += 1;
        let tick = self.tick;
        let (cover, seen) = self.decoded.get_mut(key)?;
        *seen = tick;
        Some(cover.clone())
    }

    /// Asks the thread for `key` ahead of everything else, once.
    pub fn request(&mut self, key: &str) {
        if !self.queue(key) {
            return;
        }
        let worker = self.worker.get_or_insert_with(Worker::start);
        if worker.wanted.send(key.to_string()).is_ok() {
            worker.outstanding += 1;
            worker.wanted_outstanding.insert(key.to_string());
        }
    }

    /// Hands every key the window is not showing yet to the thread.
    ///
    /// This is what fills the cache behind the visible rows. The queue is
    /// worked through only while nothing waits to be drawn, so a picture the
    /// window needs still arrives next.
    pub fn prefetch(&mut self, keys: impl IntoIterator<Item = String>) {
        for key in keys {
            if !self.queue(&key) {
                continue;
            }
            let worker = self.worker.get_or_insert_with(Worker::start);
            if worker.background.send(key).is_ok() {
                worker.outstanding += 1;
            }
        }
    }

    /// Whether `key` still needs to be decoded, noting that it was asked for.
    fn queue(&mut self, key: &str) -> bool {
        if self.decoded.contains_key(key) || !self.requested.insert(key.to_string()) {
            return false;
        }
        self.worker.get_or_insert_with(Worker::start);
        true
    }

    /// Takes in every picture that has come back, and says whether one did.
    pub fn poll(&mut self) -> bool {
        let Some(worker) = &mut self.worker else {
            return false;
        };
        let mut arrived = false;
        while let Ok((key, cover)) = worker.results.try_recv() {
            worker.outstanding = worker.outstanding.saturating_sub(1);
            worker.wanted_outstanding.remove(&key);
            self.tick += 1;
            let tick = self.tick;
            self.decoded.insert(key, (cover, tick));
            arrived = true;
        }
        if arrived {
            self.forget_the_oldest();
        }
        arrived
    }

    /// Drops the pictures left alone longest once too many are kept.
    fn forget_the_oldest(&mut self) {
        while self.decoded.len() > KEPT {
            let Some(oldest) = self
                .decoded
                .iter()
                .min_by_key(|(_, (_, seen))| *seen)
                .map(|(key, _)| key.clone())
            else {
                return;
            };
            self.decoded.remove(&oldest);
            // The key may be asked for again, and this time it is worth
            // decoding: what dropped it was room, not a missing picture.
            self.requested.remove(&oldest);
        }
    }

    /// Whether a picture the drawing pass asked for is still being decoded.
    ///
    /// Only those make the window repaint: work behind them fills the cache
    /// without costing frames, and arrives when something else draws.
    pub fn pending(&self) -> bool {
        self.worker
            .as_ref()
            .is_some_and(|worker| !worker.wanted_outstanding.is_empty())
    }
}

impl Worker {
    /// Starts the thread, which ends when the app drops its end of the queue.
    fn start() -> Self {
        let (wanted, wanted_queue) = mpsc::channel::<String>();
        let (background, background_queue) = mpsc::channel::<String>();
        let (sender, results) = mpsc::channel();
        thread::spawn(move || {
            while let Some(key) = next(&wanted_queue, &background_queue) {
                let cover = decode_now(&key).map(Arc::new);
                if sender.send((key, cover)).is_err() {
                    break;
                }
            }
        });
        Self {
            wanted,
            background,
            results,
            outstanding: 0,
            wanted_outstanding: HashSet::new(),
        }
    }
}

/// The next key to decode: whatever the window is waiting for, else the queue
/// behind it. `None` once both queues are closed and empty.
fn next(wanted: &Receiver<String>, background: &Receiver<String>) -> Option<String> {
    loop {
        match wanted.try_recv() {
            Ok(key) => return Some(key),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => return background.recv().ok(),
        }
        match background.recv_timeout(PEEK) {
            Ok(key) => return Some(key),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return wanted.recv().ok(),
        }
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
        let mut art = Art::default();
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
        assert!(
            !art.pending(),
            "the answer is kept, so the key is not asked for again"
        );
    }

    #[test]
    fn a_key_handed_to_the_background_is_answered_without_being_drawn() {
        let mut art = Art::default();
        let key = "0000000000000000-0";
        art.prefetch([key.to_string()]);
        assert!(
            !art.pending(),
            "work the window is not waiting for costs it no frames"
        );
        settle(&mut art, key);

        assert!(
            art.decoded(key).is_some_and(|cover| cover.is_none()),
            "a picture nobody drew is decoded too"
        );
        assert!(!art.pending(), "and the thread is idle again");
    }

    #[test]
    fn a_key_the_window_asked_for_is_what_pending_counts() {
        let mut art = Art::default();
        art.request("0000000000000000-0");
        assert!(art.pending(), "the window has a picture to wait for");
        settle(&mut art, "0000000000000000-0");

        assert!(!art.pending(), "and nothing is left to wait for");
    }
}
