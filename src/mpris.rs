//! MPRIS control over D-Bus, so players such as playerctl can drive the app.
//!
//! MPRIS is the interface a desktop's media keys and a player's remote
//! control speak. It runs over the session bus, which a host may not have:
//! there is no bus in a container, on a server, or when the desktop does not
//! run one. [`spawn`] therefore reports whether a service came up, and the
//! window works exactly as it does without one when none did.
//!
//! The service runs on its own thread, so a bus that is slow to answer never
//! holds up a frame. Commands come back over a channel, which the window
//! drains once a frame; the state it publishes goes the other way, through a
//! lock the service reads when a client asks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

/// The bus name MPRIS gives a player, which playerctl looks for.
const BUS_NAME: &str = "org.mpris.MediaPlayer2.blue_dove";

/// The object path every MPRIS player serves at.
const OBJECT_PATH: &str = "/org/mpris/MediaPlayer2";

/// The interface the transport lives on.
const PLAYER_INTERFACE: &str = "org.mpris.MediaPlayer2.Player";

/// The interface a property change is announced on.
const PROPERTIES_INTERFACE: &str = "org.freedesktop.DBus.Properties";

/// What a client asked the player to do.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Command {
    /// Start or resume playback.
    Play,
    /// Pause playback.
    Pause,
    /// Play when paused, pause when playing.
    PlayPause,
    /// Stop playback.
    Stop,
    /// Move to the next track.
    Next,
    /// Move to the previous track.
    Previous,
    /// Move the position by this many microseconds.
    Seek(i64),
    /// Move to this position, in microseconds.
    SetPosition(i64),
    /// Set the output volume, from 0 to 1.
    SetVolume(f64),
    /// Turn repeating the playing track on or off.
    SetRepeat(bool),
    /// Turn random order on or off.
    SetShuffle(bool),
    /// Bring the window to the front.
    Raise,
}

/// What the player is doing, as MPRIS names it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Playback {
    /// Nothing is playing.
    #[default]
    Stopped,
    /// A track is loaded and paused.
    Paused,
    /// A track is playing.
    Playing,
}

impl Playback {
    /// The name MPRIS uses for this state.
    fn name(self) -> &'static str {
        match self {
            Playback::Stopped => "Stopped",
            Playback::Paused => "Paused",
            Playback::Playing => "Playing",
        }
    }
}

/// What the window tells a client about itself.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    /// Whether anything plays, and whether it is paused.
    pub playback: Playback,
    /// Title of the playing track.
    pub title: String,
    /// Its artists.
    pub artists: Vec<String>,
    /// Its album.
    pub album: String,
    /// Its length in microseconds.
    pub length: i64,
    /// Where playback is, in microseconds.
    pub position: i64,
    /// Output volume, from 0 to 1.
    pub volume: f64,
    /// Whether the playing track repeats.
    pub repeat: bool,
    /// Whether the next track is picked at random.
    pub shuffle: bool,
    /// Whether the track can be seeked in.
    pub can_seek: bool,
    /// Whether there is a next track.
    pub can_go_next: bool,
    /// Whether there is a previous track.
    pub can_go_previous: bool,
    /// Which track is loaded, for the metadata's track id.
    pub track: u64,
    /// The PNG of the playing track's cover, which a media widget draws.
    ///
    /// MPRIS names art by a URI rather than by pixels, so the window writes
    /// the cover out and hands over its path.
    pub art: Option<PathBuf>,
}

/// A running MPRIS service.
pub struct Service {
    /// The connection the interfaces are served on.
    connection: Connection,
    /// The state clients are answered from.
    state: Arc<Mutex<State>>,
}

impl Service {
    /// Publishes `state` to clients.
    ///
    /// The position changes every frame and is not announced, since clients
    /// ask for it when they need it; everything else is announced once, when
    /// it changes.
    pub fn update(&self, state: State) {
        let Ok(mut shared) = self.state.lock() else {
            return;
        };
        let previous = shared.clone();
        if previous == state {
            return;
        }
        *shared = state.clone();
        drop(shared);

        let mut changed: HashMap<&str, OwnedValue> = HashMap::new();
        if (previous.title != state.title
            || previous.artists != state.artists
            || previous.album != state.album
            || previous.length != state.length
            || previous.track != state.track
            || previous.art != state.art)
            && let Ok(value) = Value::from(metadata(&state)).try_to_owned()
        {
            changed.insert("Metadata", value);
        }

        let boolean = |value: bool| Value::from(value).try_to_owned().ok();
        let mut note = |name: &'static str, different: bool, value: Option<OwnedValue>| {
            if different && let Some(value) = value {
                changed.insert(name, value);
            }
        };
        note(
            "PlaybackStatus",
            previous.playback != state.playback,
            Some(text(state.playback.name())),
        );
        note(
            "Volume",
            previous.volume != state.volume,
            Some(volume(state.volume)),
        );
        note(
            "LoopStatus",
            previous.repeat != state.repeat,
            Some(text(loop_status(state.repeat))),
        );
        note(
            "Shuffle",
            previous.shuffle != state.shuffle,
            boolean(state.shuffle),
        );
        note(
            "CanSeek",
            previous.can_seek != state.can_seek,
            boolean(state.can_seek),
        );
        note(
            "CanGoNext",
            previous.can_go_next != state.can_go_next,
            boolean(state.can_go_next),
        );
        note(
            "CanGoPrevious",
            previous.can_go_previous != state.can_go_previous,
            boolean(state.can_go_previous),
        );
        if !changed.is_empty() {
            let _ = self.announce(changed);
        }

        // A jump in position is a seek, which MPRIS announces of its own.
        let jumped = (state.position - previous.position).abs() > SEEK_JUMP;
        if jumped && state.playback == Playback::Playing && previous.playback == Playback::Playing {
            let _ = self.connection.emit_signal(
                None::<&str>,
                OBJECT_PATH,
                PLAYER_INTERFACE,
                "Seeked",
                &state.position,
            );
        }
    }

    /// Announces the properties that changed, as the MPRIS spec has it.
    fn announce(&self, changed: HashMap<&str, OwnedValue>) -> zbus::Result<()> {
        let body = (PLAYER_INTERFACE, changed, Vec::<&str>::new());
        self.connection.emit_signal(
            None::<&str>,
            OBJECT_PATH,
            PROPERTIES_INTERFACE,
            "PropertiesChanged",
            &body,
        )
    }
}

/// Something to call when a command arrives, so the window wakes up.
///
/// The window only draws when it is asked to, so without this a command
/// would sit in the channel until the user moved the mouse.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// A jump in position larger than this, in microseconds, is a seek.
const SEEK_JUMP: i64 = 1_500_000;

/// Starts the service on a thread of its own.
///
/// Answers the channel commands arrive on, and one that yields the service
/// once the bus has been connected, or `None` when there is no session bus,
/// or another player already holds the name. Nothing about the window waits
/// for either.
pub fn spawn(waker: Waker) -> (Receiver<Command>, Receiver<Option<Service>>) {
    let (commands, command_receiver) = mpsc::channel();
    let (services, service_receiver) = mpsc::channel();
    let thread_services = services.clone();
    let spawned = std::thread::Builder::new()
        .name("mpris".to_owned())
        .spawn(move || {
            let service = connect(commands, waker).ok();
            let _ = thread_services.send(service);
        });
    if spawned.is_err() {
        let _ = services.send(None);
    }
    (command_receiver, service_receiver)
}

/// Connects to the session bus and serves the MPRIS interfaces on it.
fn connect(commands: Sender<Command>, waker: Waker) -> zbus::Result<Service> {
    let state = Arc::new(Mutex::new(State::default()));
    let player = Player {
        state: Arc::clone(&state),
        commands: Mutex::new(commands.clone()),
        waker: Arc::clone(&waker),
    };
    let root = Root {
        commands: Mutex::new(commands),
        waker,
    };
    // zbus asks for a name with AllowReplacement and ReplaceExisting by default,
    // which would let a second window take the name from the first, or let any
    // other process take it from this one. Neither is wanted: the player that
    // already answers keeps answering, and the newcomer runs without remote
    // control.
    let connection = zbus::blocking::connection::Builder::session()?
        .allow_name_replacements(false)
        .replace_existing_names(false)
        .name(BUS_NAME)?
        .serve_at(OBJECT_PATH, player)?
        .serve_at(OBJECT_PATH, root)?
        .build()?;
    Ok(Service { connection, state })
}

/// The `org.mpris.MediaPlayer2.Player` interface.
struct Player {
    /// The state the properties are answered from.
    state: Arc<Mutex<State>>,
    /// Where commands go.
    commands: Mutex<Sender<Command>>,
    /// Called after a command, so the window draws and takes it in.
    waker: Waker,
}

impl Player {
    /// Sends a command to the window, ignoring a window that has gone.
    fn send(&self, command: Command) {
        if let Ok(commands) = self.commands.lock() {
            let _ = commands.send(command);
        }
        (self.waker)();
    }

    /// A copy of the state, or the default when the lock is poisoned.
    fn state(&self) -> State {
        self.state
            .lock()
            .map(|state| state.clone())
            .unwrap_or_default()
    }
}

#[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
impl Player {
    /// Starts or resumes playback.
    fn play(&self) {
        self.send(Command::Play);
    }

    /// Pauses playback.
    fn pause(&self) {
        self.send(Command::Pause);
    }

    /// Plays when paused, pauses when playing.
    fn play_pause(&self) {
        self.send(Command::PlayPause);
    }

    /// Stops playback.
    fn stop(&self) {
        self.send(Command::Stop);
    }

    /// Moves to the next track.
    fn next(&self) {
        self.send(Command::Next);
    }

    /// Moves to the previous track.
    fn previous(&self) {
        self.send(Command::Previous);
    }

    /// Moves the position by `offset`, in microseconds.
    fn seek(&self, offset: i64) {
        self.send(Command::Seek(offset));
    }

    /// Moves to `position`, in microseconds, in the track `track_id`.
    fn set_position(&self, track_id: ObjectPath<'_>, position: i64) {
        // A client may name the track it thinks is loaded; the window seeks
        // in whatever it has, which is what a stale id would want anyway.
        let _ = track_id;
        self.send(Command::SetPosition(position));
    }

    /// Whether anything plays, and whether it is paused.
    #[zbus(property)]
    fn playback_status(&self) -> String {
        self.state().playback.name().to_owned()
    }

    /// Whether the track repeats, as MPRIS spells it.
    #[zbus(property)]
    fn loop_status(&self) -> String {
        loop_status(self.state().repeat).to_owned()
    }

    /// Turns repeating on or off.
    #[zbus(property)]
    fn set_loop_status(&self, status: String) {
        self.send(Command::SetRepeat(status == "Track"));
    }

    /// Whether the next track is picked at random.
    #[zbus(property)]
    fn shuffle(&self) -> bool {
        self.state().shuffle
    }

    /// Turns random order on or off.
    #[zbus(property)]
    fn set_shuffle(&self, on: bool) {
        self.send(Command::SetShuffle(on));
    }

    /// The playing track's details.
    #[zbus(property)]
    fn metadata(&self) -> HashMap<String, OwnedValue> {
        metadata(&self.state())
    }

    /// The output volume, from 0 to 1.
    #[zbus(property)]
    fn volume(&self) -> f64 {
        self.state().volume
    }

    /// Sets the output volume.
    #[zbus(property)]
    fn set_volume(&self, volume: f64) {
        self.send(Command::SetVolume(volume));
    }

    /// Where playback is, in microseconds.
    #[zbus(property)]
    fn position(&self) -> i64 {
        self.state().position
    }

    /// The playback rate, which is always normal speed here.
    #[zbus(property)]
    fn rate(&self) -> f64 {
        1.0
    }

    /// The slowest playback rate, which is normal speed here.
    #[zbus(property)]
    fn minimum_rate(&self) -> f64 {
        1.0
    }

    /// The fastest playback rate, which is normal speed here.
    #[zbus(property)]
    fn maximum_rate(&self) -> f64 {
        1.0
    }

    /// Whether the player can start a track.
    #[zbus(property)]
    fn can_play(&self) -> bool {
        true
    }

    /// Whether the player can pause.
    #[zbus(property)]
    fn can_pause(&self) -> bool {
        true
    }

    /// Whether there is a next track.
    #[zbus(property)]
    fn can_go_next(&self) -> bool {
        self.state().can_go_next
    }

    /// Whether there is a previous track.
    #[zbus(property)]
    fn can_go_previous(&self) -> bool {
        self.state().can_go_previous
    }

    /// Whether the track can be seeked in.
    #[zbus(property)]
    fn can_seek(&self) -> bool {
        self.state().can_seek
    }

    /// Whether a client may control the player at all.
    #[zbus(property)]
    fn can_control(&self) -> bool {
        true
    }
}

/// The `org.mpris.MediaPlayer2` interface, which names the player.
struct Root {
    /// Where commands go.
    commands: Mutex<Sender<Command>>,
    /// Called after a command, so the window draws and takes it in.
    waker: Waker,
}

#[zbus::interface(name = "org.mpris.MediaPlayer2")]
impl Root {
    /// Brings the window to the front.
    fn raise(&self) {
        if let Ok(commands) = self.commands.lock() {
            let _ = commands.send(Command::Raise);
        }
        (self.waker)();
    }

    /// The name a desktop shows for this player.
    #[zbus(property)]
    fn identity(&self) -> String {
        "Blue Dove".to_owned()
    }

    /// The desktop entry that names its icon.
    #[zbus(property)]
    fn desktop_entry(&self) -> String {
        "blue-dove".to_owned()
    }

    /// Whether a client may close the player.
    #[zbus(property)]
    fn can_quit(&self) -> bool {
        false
    }

    /// Whether a client may bring the window to the front.
    #[zbus(property)]
    fn can_raise(&self) -> bool {
        true
    }

    /// Whether the player has a track list of its own.
    #[zbus(property)]
    fn has_track_list(&self) -> bool {
        false
    }

    /// The URI schemes the player opens.
    #[zbus(property)]
    fn supported_uri_schemes(&self) -> Vec<String> {
        Vec::new()
    }

    /// The MIME types the player opens.
    #[zbus(property)]
    fn supported_mime_types(&self) -> Vec<String> {
        Vec::new()
    }
}

/// The `LoopStatus` MPRIS gives for a repeat setting.
fn loop_status(repeat: bool) -> &'static str {
    if repeat { "Track" } else { "None" }
}

/// The metadata map MPRIS expects for the playing track.
fn metadata(state: &State) -> HashMap<String, OwnedValue> {
    let mut metadata = HashMap::new();
    metadata.insert("mpris:trackid".to_owned(), track_id(state.track));
    metadata.insert("mpris:length".to_owned(), number(state.length));
    metadata.insert("xesam:title".to_owned(), text(&state.title));
    metadata.insert(
        "xesam:artist".to_owned(),
        Value::from(state.artists.clone())
            .try_to_owned()
            .unwrap_or_else(|_| text("")),
    );
    metadata.insert("xesam:album".to_owned(), text(&state.album));
    if let Some(art) = &state.art {
        metadata.insert("mpris:artUrl".to_owned(), text(&file_url(art)));
    }
    metadata
}

/// The `file://` URI a path is named by, which is what MPRIS takes for art.
///
/// Everything but an unreserved character is percent-encoded, so a cover in a
/// directory with a space or a non-ASCII name still reads as one URI. MPRIS
/// exists only where a session bus does, so the paths are the Unix ones: a
/// Windows path would need its separators and its drive letter rewritten.
fn file_url(path: &Path) -> String {
    let mut url = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' => {
                url.push(char::from(*byte));
            }
            _ => url.push_str(&format!("%{byte:02X}")),
        }
    }
    url
}

/// The object path naming track `track`, which MPRIS requires.
fn track_id(track: u64) -> OwnedValue {
    let path = ObjectPath::try_from(format!("/org/blue_dove/track/{track}"))
        .unwrap_or_else(|_| ObjectPath::from_static_str_unchecked("/org/blue_dove/track/0"));
    let path = OwnedObjectPath::from(path);
    Value::from(path)
        .try_to_owned()
        .unwrap_or_else(|_| text(""))
}

/// A string as a D-Bus value.
fn text(value: &str) -> OwnedValue {
    Value::from(value.to_owned())
        .try_to_owned()
        .unwrap_or_else(|_| {
            Value::from(String::new())
                .try_to_owned()
                .expect("a string is a value")
        })
}

/// A number as a D-Bus value.
///
/// MPRIS spells a length or a position as a 64-bit integer, so this is for
/// those; a volume is a double and goes through [`volume`].
fn number(value: i64) -> OwnedValue {
    Value::from(value)
        .try_to_owned()
        .unwrap_or_else(|_| text(""))
}

/// A volume as a D-Bus value, which MPRIS spells as a double.
fn volume(value: f64) -> OwnedValue {
    Value::from(value)
        .try_to_owned()
        .unwrap_or_else(|_| text(""))
}

#[cfg(test)]
mod tests {
    use std::sync::MutexGuard;
    use std::time::Duration;

    use super::*;

    fn state() -> State {
        State {
            playback: Playback::Playing,
            title: "Main Theme".to_owned(),
            artists: vec!["Koji Kondo".to_owned()],
            album: "Super Mario 64".to_owned(),
            length: 62_000_000,
            position: 5_000_000,
            volume: 0.8,
            repeat: false,
            shuffle: false,
            can_seek: true,
            can_go_next: true,
            can_go_previous: true,
            track: 3,
            art: None,
        }
    }

    #[test]
    fn the_playback_states_are_named_as_mpris_names_them() {
        assert_eq!(Playback::Stopped.name(), "Stopped");
        assert_eq!(Playback::Paused.name(), "Paused");
        assert_eq!(Playback::Playing.name(), "Playing");
        assert_eq!(loop_status(true), "Track");
        assert_eq!(loop_status(false), "None");
    }

    #[test]
    fn the_metadata_carries_what_a_client_shows() {
        let metadata = metadata(&state());
        assert_eq!(
            metadata["xesam:title"],
            text("Main Theme"),
            "the title is there"
        );
        assert_eq!(metadata["xesam:album"], text("Super Mario 64"));
        assert_eq!(
            metadata["xesam:artist"],
            Value::from(vec!["Koji Kondo".to_owned()])
                .try_to_owned()
                .expect("an array of strings is a value")
        );
        assert_eq!(metadata["mpris:length"], number(62_000_000));
        assert_eq!(
            metadata["mpris:length"].value_signature().to_string(),
            "x",
            "MPRIS spells a length as a 64-bit integer"
        );
        assert_eq!(
            metadata["mpris:trackid"],
            track_id(3),
            "the track id names the track"
        );
        assert!(
            metadata["mpris:trackid"].value_signature().to_string() == "o",
            "a track id is an object path, which playerctl requires"
        );
    }

    #[test]
    fn a_track_id_is_a_valid_object_path_for_any_track() {
        for track in [0, 1, 4_294_967_295] {
            let id = track_id(track);
            assert_eq!(id.value_signature().to_string(), "o");
        }
    }

    #[test]
    fn a_stopped_player_has_no_metadata_to_show() {
        let metadata = metadata(&State::default());
        assert_eq!(metadata["xesam:title"], text(""));
        assert_eq!(metadata["mpris:length"], number(0));
        assert!(
            !metadata.contains_key("mpris:artUrl"),
            "nothing plays, so there is no art to name"
        );
    }

    #[test]
    fn the_metadata_names_the_art_by_a_file_uri() {
        // A media widget loads the picture from a URI, so the path is turned
        // into one, spaces and all.
        let mut state = state();
        state.art = Some(PathBuf::from(
            "/home/some one/.cache/blue-dove/art/a1b2.png",
        ));
        let metadata = metadata(&state);
        assert_eq!(
            metadata["mpris:artUrl"],
            text("file:///home/some%20one/.cache/blue-dove/art/a1b2.png"),
            "the path is a file URI a client can open"
        );
    }

    /// The service, when this host has a session bus to serve it on.
    ///
    /// A machine without one is not a failure: that is the case the app is
    /// built to keep working in, so the test says so and stops.
    ///
    /// The name a service serves under has one owner, so the tests that need
    /// a service take [`BUS`] and hold it for as long as they do; two of them
    /// running at once would leave the second without a service at all.
    fn service() -> Option<Session> {
        let bus = BUS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let (commands, receiver) = spawn(Arc::new(|| {}));
        let service = receiver.recv_timeout(Duration::from_secs(5)).ok()??;
        Some(Session {
            service,
            commands,
            _bus: bus,
        })
    }

    /// A service, the commands it sends, and the lock that keeps the name.
    struct Session {
        /// The running service.
        service: Service,
        /// The commands the service sends the window.
        commands: Receiver<Command>,
        /// Held while the service owns the bus name.
        _bus: MutexGuard<'static, ()>,
    }

    /// Serialises the tests that need the bus name.
    static BUS: Mutex<()> = Mutex::new(());

    #[test]
    fn a_client_can_read_the_player_and_drive_it() {
        let Some(Session {
            service, commands, ..
        }) = service()
        else {
            eprintln!("no session bus here; the service is not started, which is supported");
            return;
        };
        service.update(state());

        let player = zbus::blocking::Proxy::new(
            &service.connection,
            BUS_NAME,
            OBJECT_PATH,
            PLAYER_INTERFACE,
        )
        .expect("the player is on the bus");

        let status: String = player
            .get_property("PlaybackStatus")
            .expect("the status reads");
        assert_eq!(status, "Playing");
        // The metadata is a property, which a client reads through the
        // properties interface, the way playerctl does.
        let metadata: HashMap<String, OwnedValue> =
            player.get_property("Metadata").expect("the metadata reads");
        assert_eq!(metadata["xesam:title"], text("Main Theme"));
        assert_eq!(metadata["xesam:album"], text("Super Mario 64"));

        player
            .call::<_, _, ()>("Next", &())
            .expect("next is called");
        assert_eq!(
            commands.recv_timeout(Duration::from_secs(5)).ok(),
            Some(Command::Next)
        );
        player
            .call::<_, _, ()>("Pause", &())
            .expect("pause is called");
        assert_eq!(
            commands.recv_timeout(Duration::from_secs(5)).ok(),
            Some(Command::Pause)
        );
    }

    #[test]
    fn the_volume_a_client_sets_reaches_the_player() {
        let Some(Session {
            service, commands, ..
        }) = service()
        else {
            return;
        };
        service.update(state());
        let player = zbus::blocking::Proxy::new(
            &service.connection,
            BUS_NAME,
            OBJECT_PATH,
            PLAYER_INTERFACE,
        )
        .expect("the player is on the bus");
        player
            .set_property("Volume", 0.35_f64)
            .expect("the volume is set");
        let volume = commands
            .recv_timeout(Duration::from_secs(5))
            .expect("a command arrives");
        assert_eq!(volume, Command::SetVolume(0.35));
    }

    #[test]
    fn a_second_player_does_not_take_the_name_from_the_first() {
        let Some(Session {
            service, commands, ..
        }) = service()
        else {
            return;
        };
        let (_second_commands, receiver) = spawn(Arc::new(|| {}));
        assert!(
            matches!(receiver.recv_timeout(Duration::from_secs(5)), Ok(None)),
            "a second service reports that it has none, rather than taking the name"
        );

        let player = zbus::blocking::Proxy::new(
            &service.connection,
            BUS_NAME,
            OBJECT_PATH,
            PLAYER_INTERFACE,
        )
        .expect("the player is on the bus");
        player
            .call::<_, _, ()>("Next", &())
            .expect("next is called");
        assert_eq!(
            commands.recv_timeout(Duration::from_secs(5)).ok(),
            Some(Command::Next),
            "the service that had the name still answers on it"
        );
    }
}
