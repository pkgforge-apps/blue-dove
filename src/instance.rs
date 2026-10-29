//! One window per user.
//!
//! A launch takes an exclusive lock on a file of its own and listens on a
//! loopback socket; a second launch finds the lock taken, hands the window
//! that holds it whatever it was asked to do, and exits. No session bus is
//! involved, so this works on a host that has none.
//!
//! The kernel drops the lock when its process dies, so a window that crashed
//! does not leave the app taken for good.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

/// How long a second launch waits for the running window to answer.
///
/// A window between taking its lock and writing its port has not stopped
/// answering, so a refusal is worth retrying for as long as this.
const ANSWER_WAIT: Duration = Duration::from_secs(2);

/// How long to wait between attempts to reach the running window.
const RETRY: Duration = Duration::from_millis(50);

/// How a launch wakes a window that is not drawing, such as a hidden one.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// What a launch asks of the window that is already running.
///
/// It travels as text over the loopback socket: a verb on the first line, and
/// for [`Request::Open`] one path per line after it. A path holding a line
/// break cannot be sent, which [`crate::cli::parse`] refuses up front.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Bring the window back to the screen.
    Show,
    /// Play, pause, stop or step the player.
    Action(crate::cli::Action),
    /// Play these files, or queue what these directories hold.
    Open(Vec<PathBuf>),
}

impl Request {
    /// The request as the text a launch writes.
    fn encode(&self) -> String {
        match self {
            Self::Show => "show\n".to_string(),
            Self::Action(action) => format!("{}\n", action.verb()),
            Self::Open(paths) => {
                let mut text = String::from("open\n");
                for path in paths {
                    text.push_str(&path.to_string_lossy());
                    text.push('\n');
                }
                text
            }
        }
    }

    /// The request in the text a launch wrote, or `None` when it makes no
    /// sense and should be ignored.
    fn decode(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        match lines.next()? {
            "show" => Some(Self::Show),
            "open" => {
                let paths: Vec<PathBuf> = lines
                    .filter(|line| !line.is_empty())
                    .map(PathBuf::from)
                    .collect();
                (!paths.is_empty()).then_some(Self::Open(paths))
            }
            verb => crate::cli::Action::from_verb(verb).map(Self::Action),
        }
    }
}

/// What this launch is.
pub enum Launch {
    /// No other window is running, so this one is the app.
    Owner {
        /// The lock, which says this process owns the app until it exits.
        ///
        /// It must stay open: the lock goes with the file.
        lock: File,
        /// The socket a later launch asks for the window on, when one could
        /// be opened. Without it the app still runs, and a second launch
        /// simply opens a second window.
        listener: Option<TcpListener>,
    },
    /// A window was already running. The caller hands it its request and
    /// exits.
    AlreadyRunning,
}

/// Takes the app's lock, or reports that a window already holds it.
///
/// A second launch never opens a window: it gets [`Launch::AlreadyRunning`]
/// and hands its request to the running one before exiting.
pub fn claim() -> Launch {
    match directory() {
        Some(directory) => claim_in(&directory),
        // With nowhere to keep a lock, a player that opens twice is better
        // than one that never opens.
        None => open_anyway(),
    }
}

/// A launch that opens its window without a lock to keep.
///
/// Used where the lock cannot be taken at all: a second window then runs
/// beside the first, which is worse than not starting but better than the
/// app refusing to open.
fn open_anyway() -> Launch {
    Launch::Owner {
        lock: null_lock(),
        listener: TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok(),
    }
}

/// Takes the lock in `directory`, or asks the window that holds it to show.
pub fn claim_in(directory: &Path) -> Launch {
    if let Err(error) = std::fs::create_dir_all(directory) {
        eprintln!("blue-dove: cannot use {}: {error}", directory.display());
        return open_anyway();
    }

    let lock = match lock_file(directory) {
        Ok(lock) => lock,
        Err(error) => {
            eprintln!("blue-dove: cannot lock {}: {error}", directory.display());
            return open_anyway();
        }
    };

    match lock.try_lock() {
        Ok(()) => Launch::Owner {
            lock,
            listener: publish(directory),
        },
        // The lock is held, so a window is running. Handing it a request is
        // the caller's job; it may not answer, in which case it is stuck
        // rather than open, and a second window would write the same
        // settings it wrote.
        Err(TryLockError::WouldBlock) => Launch::AlreadyRunning,
        Err(TryLockError::Error(error)) => {
            eprintln!("blue-dove: cannot lock {}: {error}", directory.display());
            Launch::Owner {
                lock,
                listener: publish(directory),
            }
        }
    }
}

/// Answers every launch that reaches `listener`, passing on what it asked.
///
/// The thread runs for as long as the process does. A client that sends
/// nothing is still a launch, so it counts as [`Request::Show`]; the answer
/// tells the client the request arrived, which is what keeps it from waiting.
pub fn serve(listener: TcpListener, wake: Waker) -> Receiver<Request> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                continue;
            };
            let mut text = String::new();
            let _ = stream.read_to_string(&mut text);
            let _ = stream.write_all(b"shown\n");
            let request = Request::decode(&text).unwrap_or(Request::Show);
            if sender.send(request).is_err() {
                return;
            }
            wake();
        }
    });
    receiver
}

/// Hands `request` to the window that is running, answering whether it took it.
///
/// Answers at once when nothing holds the app's lock, so a command with no
/// player behind it reports that rather than waiting.
pub fn send(request: &Request) -> bool {
    match directory() {
        Some(directory) => send_in(&directory, request),
        None => false,
    }
}

/// Hands `request` to the window running in `directory`.
pub fn send_in(directory: &Path, request: &Request) -> bool {
    let start = Instant::now();
    loop {
        if let Some(port) = read_port(directory)
            && let Ok(mut stream) = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
        {
            let _ = stream.set_read_timeout(Some(RETRY));
            let _ = stream.write_all(request.encode().as_bytes());
            // The window reads to the end of what was written, so the write
            // half is closed to say there is no more of it.
            let _ = stream.shutdown(std::net::Shutdown::Write);
            // The answer is what tells this apart from a port some other
            // process happens to be listening on.
            let mut answer = [0u8; 6];
            return stream.read_exact(&mut answer).is_ok() && &answer == b"shown\n";
        }
        // Nobody holds the lock, so there is no window to wait for.
        if !held(directory) || start.elapsed() >= ANSWER_WAIT {
            return false;
        }
        std::thread::sleep(RETRY);
    }
}

/// Whether some process holds the app's lock in `directory`.
fn held(directory: &Path) -> bool {
    match lock_file(directory) {
        Ok(lock) => matches!(lock.try_lock(), Err(TryLockError::WouldBlock)),
        Err(_) => false,
    }
}

/// Takes the lock for a window that cannot keep one, at nothing.
fn null_lock() -> File {
    let path: PathBuf = if cfg!(windows) {
        PathBuf::from("NUL")
    } else {
        PathBuf::from("/dev/null")
    };
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap_or_else(|error| panic!("{} is always openable: {error}", path.display()))
}

/// Opens the lock file, creating it when it is not there.
fn lock_file(directory: &Path) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join("instance.lock"))
}

/// Listens for later launches and writes the port they should use.
///
/// Answers `None` when no socket could be opened; the window still runs, and
/// a later launch opens a second one.
fn publish(directory: &Path) -> Option<TcpListener> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?;
    let address = listener.local_addr().ok()?;
    if let Err(error) = std::fs::write(port_file(directory), address.port().to_string()) {
        eprintln!("blue-dove: cannot write the port: {error}");
    }
    Some(listener)
}

/// The port the running window serves on, if it has written one.
fn read_port(directory: &Path) -> Option<u16> {
    std::fs::read_to_string(port_file(directory))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The file holding the port the running window serves on.
fn port_file(directory: &Path) -> PathBuf {
    directory.join("instance.port")
}

/// The directory the lock and the port file live in.
///
/// The runtime directory is for exactly this, and is emptied when the user
/// logs out. It is missing on macOS and Windows, and on a Linux session
/// without one, so the cache directory stands in: both are per user, which is
/// what keeps two accounts from locking each other out.
fn directory() -> Option<PathBuf> {
    dirs::runtime_dir()
        .or_else(dirs::cache_dir)
        .map(|dir| dir.join("blue-dove"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// A directory of its own for one test, removed when it drops.
    struct Temporary(PathBuf);

    impl Temporary {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!("blue-dove-instance-{name}"));
            let _ = std::fs::remove_dir_all(&path);
            Self(path)
        }
    }

    impl Drop for Temporary {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_first_launch_owns_the_app() {
        let directory = Temporary::new("first");
        assert!(matches!(claim_in(&directory.0), Launch::Owner { .. }));
    }

    #[test]
    fn a_second_launch_finds_the_owner_and_sends_it_what_it_was_asked() {
        let directory = Temporary::new("second");
        let Launch::Owner {
            lock,
            listener: Some(listener),
        } = claim_in(&directory.0)
        else {
            panic!("the first launch owns the app and listens");
        };
        let woken = Arc::new(AtomicBool::new(false));
        let flag = woken.clone();
        let requests = serve(
            listener,
            Arc::new(move || flag.store(true, Ordering::SeqCst)),
        );

        assert!(
            matches!(claim_in(&directory.0), Launch::AlreadyRunning),
            "the second launch must not open a window"
        );
        assert!(
            send_in(&directory.0, &Request::Action(crate::cli::Action::Next)),
            "the running window takes the request"
        );
        assert_eq!(
            requests
                .recv_timeout(ANSWER_WAIT)
                .expect("the request arrives"),
            Request::Action(crate::cli::Action::Next)
        );
        assert!(wait_for(&woken), "the window is woken to act on it");

        // The owner releases the lock as it exits, and the next launch owns it.
        lock.unlock().expect("releases the lock");
        assert!(matches!(claim_in(&directory.0), Launch::Owner { .. }));
    }

    #[test]
    fn a_path_travels_to_the_running_window() {
        let directory = Temporary::new("open");
        let Launch::Owner {
            listener: Some(listener),
            ..
        } = claim_in(&directory.0)
        else {
            panic!("the first launch owns the app and listens");
        };
        let requests = serve(listener, Arc::new(|| {}));

        let paths = vec![
            PathBuf::from("/music/one.flac"),
            PathBuf::from("/music/two.flac"),
        ];
        assert!(send_in(&directory.0, &Request::Open(paths.clone())));
        assert_eq!(
            requests
                .recv_timeout(ANSWER_WAIT)
                .expect("the request arrives"),
            Request::Open(paths)
        );
    }

    #[test]
    fn a_request_survives_the_text_it_travels_as() {
        for request in [
            Request::Show,
            Request::Action(crate::cli::Action::Toggle),
            Request::Action(crate::cli::Action::Previous),
            Request::Open(vec![PathBuf::from("/music/a file.flac")]),
        ] {
            assert_eq!(Request::decode(&request.encode()), Some(request));
        }
        assert_eq!(Request::decode(""), None);
        assert_eq!(
            Request::decode("nonsense\n"),
            None,
            "an unknown verb is ignored"
        );
        assert_eq!(Request::decode("open\n"), None, "no path is no request");
    }

    #[test]
    fn a_command_with_no_player_behind_it_answers_at_once() {
        let directory = Temporary::new("nobody");
        std::fs::create_dir_all(&directory.0).expect("makes the directory");
        let start = Instant::now();
        assert!(!send_in(&directory.0, &Request::Show));
        assert!(
            start.elapsed() < ANSWER_WAIT,
            "nothing holds the lock, so there is nothing to wait for"
        );
    }

    #[test]
    fn a_lock_left_by_a_dead_window_does_not_block_the_next_launch() {
        let directory = Temporary::new("stale");
        std::fs::create_dir_all(&directory.0).expect("makes the directory");
        // A port file left behind by a window that died, and no lock on it.
        std::fs::write(port_file(&directory.0), "1").expect("writes a stale port");
        drop(lock_file(&directory.0).expect("opens the lock"));
        assert!(matches!(claim_in(&directory.0), Launch::Owner { .. }));
    }

    /// Waits for a flag another thread sets, answering whether it did.
    fn wait_for(flag: &AtomicBool) -> bool {
        let start = Instant::now();
        while start.elapsed() < ANSWER_WAIT {
            if flag.load(Ordering::SeqCst) {
                return true;
            }
            std::thread::sleep(RETRY);
        }
        false
    }
}
