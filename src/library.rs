//! The music library: tags read from files on disk, grouped into albums.
//!
//! Scanning runs on a thread of its own and reports over a channel, so a
//! large directory never blocks the UI.

use std::collections::hash_map::DefaultHasher;
use std::collections::{BTreeMap, HashMap};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use lofty::prelude::*;
use walkdir::WalkDir;

/// File extensions the player reads.
///
/// Most decode through symphonia; see `decode` for the formats that get a
/// decoder of their own. DSD is not supported, and neither are TAK and WMA,
/// which have no decoder that fits the licence.
const AUDIO_EXTENSIONS: &[&str] = &[
    "aac", "adx", "aif", "aiff", "amr", "ape", "bcstm", "bfstm", "brstm", "caf", "dsp", "flac",
    "m4a", "m4b", "mka", "mp+", "mp1", "mp2", "mp3", "mp4", "mpc", "mpp", "oga", "ogg", "opus",
    "shn", "swag", "tta", "vag", "vig", "wav", "wv",
];

/// File names looked up for album art when the tracks embed none.
const COVER_NAMES: &[&str] = &[
    "cover.jpg",
    "cover.jpeg",
    "cover.png",
    "cover.svg",
    "folder.jpg",
    "folder.png",
    "folder.svg",
    "front.jpg",
    "front.png",
    "front.svg",
    "album.jpg",
    "albumart.jpg",
];

/// Starts of the file names an album's front cover usually has.
///
/// A rip that ships a booklet names its images `Cover1.jpg`, `front.png` or
/// `booklet front.png`; the first of those that is a front view is the art
/// the album list should show.
const COVER_PREFIXES: &[&str] = &["cover", "front", "folder", "album"];

/// Directories a rip puts its artwork in, rather than beside the tracks.
const ART_DIRECTORIES: &[&str] = &[
    "covers", "cover", "artwork", "art", "scans", "scan", "images", "booklet",
];

/// Extensions an image file can carry.
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "svg"];

/// Files scanned between two progress messages.
const PROGRESS_EVERY: usize = 200;

/// One playable file and the tags read from it.
#[derive(Clone, Debug)]
pub struct Track {
    /// Where the file lives.
    pub path: PathBuf,
    /// Track title, or the file name when the file carries none.
    pub title: String,
    /// Performing artist, or "Unknown Artist".
    pub artist: String,
    /// Album title, or the directory name when the file carries none.
    pub album: String,
    /// Album artist, which decides which album a track belongs to.
    pub album_artist: String,
    /// Release year, when the tags have one.
    pub year: Option<u32>,
    /// Disc number, when the tags have one.
    pub disc: Option<u32>,
    /// Track number, when the tags have one.
    pub number: Option<u32>,
    /// Playing time, as the tags report it.
    pub duration: Duration,
    /// Cover art from the file's tags, decoded to RGBA.
    pub cover: Option<Arc<Cover>>,
    /// Directory the file sits in, relative to the music directory.
    pub directory: PathBuf,
    /// Whether the tags named an album.
    ///
    /// A file without one still shows the directory name as its album, but it
    /// is a loose track: the album list shows it on a small row of its own
    /// rather than folding it into an album.
    pub tagged_album: bool,
}

impl Track {
    /// Text shown for the album column: the album, with its year when known.
    pub fn album_label(&self) -> String {
        match self.year {
            Some(year) => format!("{} ({year})", self.album),
            None => self.album.clone(),
        }
    }

    /// The name of the file itself, which the file name column shows.
    ///
    /// `extension` keeps the extension, such as `.flac`, so a directory of mixed
    /// formats says what each file is; without it the name reads the way a
    /// title does.
    pub fn file_name(&self, extension: bool) -> String {
        let Some(name) = self.path.file_name() else {
            return self.title.clone();
        };
        if extension {
            return name.to_string_lossy().into_owned();
        }
        self.path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.to_string_lossy().into_owned())
    }
}

/// An album cover decoded to RGBA, ready to become a texture.
pub use crate::cover::Cover;

/// Tracks that share an album, in playing order.
#[derive(Clone)]
pub struct Album {
    /// Album title.
    pub title: String,
    /// Album artist, or the artist of its first track.
    pub artist: String,
    /// Release year, when the tracks agree on one.
    pub year: Option<u32>,
    /// Indices into [`Library::tracks`], in playing order.
    pub tracks: Vec<usize>,
    /// Cover art, taken from the first track that carries any.
    pub cover: Option<Arc<Cover>>,
    /// Directory the album was found in, relative to the music directory.
    pub directory: PathBuf,
}

impl Album {
    /// Total playing time of the album.
    #[cfg(test)]
    pub fn duration(&self, library: &Library) -> Duration {
        self.tracks
            .iter()
            .filter_map(|&index| library.tracks.get(index))
            .map(|track| track.duration)
            .sum()
    }
}

/// Every track found under the music directory, the albums they form, and the
/// directories they were found in.
#[derive(Default)]
pub struct Library {
    /// Every track, in the order the scan found them.
    pub tracks: Vec<Track>,
    /// Albums, sorted by title.
    pub albums: Vec<Album>,
    /// Top-level directories under the music directory, sorted by name.
    pub directories: Vec<Directory>,
}

impl Library {
    /// Groups tracks into albums, places them in their directories, and sorts.
    ///
    /// Tracks are ordered by disc and track number, then by title, so an
    /// album plays in the order it was released. Albums are ordered by title,
    /// then year, so two releases of one name stay together.
    pub fn from_tracks(tracks: Vec<Track>) -> Self {
        let mut groups: BTreeMap<(String, String, Option<u32>), Vec<usize>> = BTreeMap::new();
        for (index, track) in tracks.iter().enumerate() {
            // A file with no album tag is loose, not an album of its own:
            // its directory name stands in for the album title only so the
            // track table has something to show.
            if !track.tagged_album {
                continue;
            }
            let key = (
                track.album_artist.to_lowercase(),
                track.album.to_lowercase(),
                track.year,
            );
            groups.entry(key).or_default().push(index);
        }

        let mut albums: Vec<Album> = groups
            .into_iter()
            .map(|((_, _, year), mut indices)| {
                indices.sort_by(|&a, &b| order(&tracks[a], &tracks[b]));
                let first = &tracks[indices[0]];
                let cover = indices
                    .iter()
                    .find_map(|&index| tracks[index].cover.clone());
                Album {
                    title: first.album.clone(),
                    artist: first.album_artist.clone(),
                    year,
                    directory: first.directory.clone(),
                    tracks: indices,
                    cover,
                }
            })
            .collect();
        albums.sort_by(|a, b| {
            a.title
                .to_lowercase()
                .cmp(&b.title.to_lowercase())
                .then(a.year.cmp(&b.year))
        });

        let directories = Directory::build(&tracks, &albums);
        Self {
            tracks,
            albums,
            directories,
        }
    }

    /// Whether the scan found nothing.
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    /// The directory at `path`, looked up through the tree.
    pub fn directory(&self, path: &Path) -> Option<&Directory> {
        fn find<'a>(directory: &'a Directory, path: &Path) -> Option<&'a Directory> {
            if directory.path == path {
                return Some(directory);
            }
            directory
                .children
                .iter()
                .find_map(|child| find(child, path))
        }
        self.directories.iter().find_map(|root| find(root, path))
    }

    /// Every album's tracks in the library, in the order the album list shows
    /// them.
    ///
    /// That is each top-level directory in turn, each as
    /// [`Directory::album_tracks`] walks it, so adding the whole library
    /// queues it in the order it is drawn.
    pub fn album_tracks(&self) -> Vec<usize> {
        let mut found = Vec::new();
        for directory in &self.directories {
            found.extend(directory.album_tracks(&self.albums));
        }
        found
    }

    /// The album each track belongs to, by track index.
    ///
    /// `None` is a loose file, which belongs to no album. The track table
    /// uses this to head each run of an album's tracks with the album, so a
    /// playlist of several albums reads as a list of albums rather than one
    /// undivided column of titles.
    pub fn album_indices(&self) -> Vec<Option<usize>> {
        let mut indices = vec![None; self.tracks.len()];
        for (album, entry) in self.albums.iter().enumerate() {
            for &track in &entry.tracks {
                if let Some(slot) = indices.get_mut(track) {
                    *slot = Some(album);
                }
            }
        }
        indices
    }
}

/// A directory of the scanned tree, as the album list shows it.
#[derive(Clone, Debug, Default)]
pub struct Directory {
    /// Directory name, without its parent path.
    pub name: String,
    /// Path relative to the music directory, which is empty for the root.
    pub path: PathBuf,
    /// Albums whose files sit directly in this directory, by library index.
    pub albums: Vec<usize>,
    /// Loose tracks in this directory: files that belong to no album.
    pub loose: Vec<usize>,
    /// Subdirectories, sorted by name.
    pub children: Vec<Directory>,
}

impl Directory {
    /// Builds the directory tree from where the tracks were found.
    ///
    /// A track belongs to an album when its tags name one; a file whose
    /// tags carry no album title is loose, and the album list shows it
    /// under its directory in a smaller row.
    fn build(tracks: &[Track], albums: &[Album]) -> Vec<Directory> {
        let mut roots: Vec<Directory> = Vec::new();
        for (index, album) in albums.iter().enumerate() {
            place(&mut roots, &album.directory, |directory| {
                directory.albums.push(index)
            });
        }
        for (index, track) in tracks.iter().enumerate() {
            if !track.tagged_album {
                place(&mut roots, &track.directory, |directory| {
                    directory.loose.push(index)
                });
            }
        }
        sort_tree(&mut roots, albums);
        roots
    }

    /// Every track in this directory and below it, in the order the album list
    /// shows them.
    ///
    /// That is each album's tracks in playing order, then the loose files
    /// sitting beside them, then everything in each subdirectory in turn, so a
    /// directory queued by a double-click reads down the list as it is drawn.
    pub fn tracks_deep(&self, albums: &[Album]) -> Vec<usize> {
        let mut found = Vec::new();
        for &index in &self.albums {
            if let Some(album) = albums.get(index) {
                found.extend(album.tracks.iter().copied());
            }
        }
        found.extend(self.loose.iter().copied());
        for child in &self.children {
            found.extend(child.tracks_deep(albums));
        }
        found
    }

    /// Every album's tracks in this directory and below it, in the order the
    /// album list shows them.
    ///
    /// The loose files are left out: they belong to no album, and the album
    /// list gives them a row of their own.
    pub fn album_tracks(&self, albums: &[Album]) -> Vec<usize> {
        let mut found = Vec::new();
        for &index in &self.albums {
            if let Some(album) = albums.get(index) {
                found.extend(album.tracks.iter().copied());
            }
        }
        for child in &self.children {
            found.extend(child.album_tracks(albums));
        }
        found
    }

    /// Whether this directory and everything below it holds nothing to show.
    pub fn is_empty(&self) -> bool {
        self.albums.is_empty()
            && self.loose.is_empty()
            && self.children.iter().all(Directory::is_empty)
    }

    /// Whether the directory is a pass-through: it leads to exactly one album
    /// and nothing else worth a row.
    ///
    /// A release usually lives in a directory of its own, and a row for that
    /// directory above its only album says nothing. The album is shown on its
    /// own instead, and a directory row appears only where it groups several.
    /// A directory tree such as `Release/Disc 1` is one chain of
    /// pass-through directories, so it collapses to the album.
    pub fn is_pass_through(&self) -> bool {
        self.loose.is_empty() && self.album_count() == 1 && !self.has_extra_rows()
    }

    /// How many albums this directory and everything below it hold.
    fn album_count(&self) -> usize {
        self.albums.len()
            + self
                .children
                .iter()
                .map(Directory::album_count)
                .sum::<usize>()
    }

    /// Whether anything below the directory needs a row of its own: a second
    /// album, a loose file, or a directory holding more than one album.
    fn has_extra_rows(&self) -> bool {
        !self.loose.is_empty()
            || self
                .children
                .iter()
                .any(|child| !child.loose.is_empty() || child.album_count() != 1)
    }
}

/// Finds or creates the directory for `path` under `roots`, then calls `add`
/// with it.
///
/// `path` is relative to the music directory, so each component is one level
/// of the tree.
fn place(roots: &mut Vec<Directory>, path: &Path, add: impl FnOnce(&mut Directory)) {
    let mut components = path.components();
    let Some(first) = components.next() else {
        return;
    };
    let name = first.as_os_str().to_string_lossy().to_string();
    let position = match roots.iter().position(|directory| directory.name == name) {
        Some(position) => position,
        None => {
            roots.push(Directory {
                name: name.clone(),
                path: PathBuf::from(&name),
                ..Directory::default()
            });
            roots.len() - 1
        }
    };

    let rest = components.as_path();
    if rest.as_os_str().is_empty() {
        add(&mut roots[position]);
    } else {
        let directory = &mut roots[position];
        let mut below = std::mem::take(&mut directory.children);
        place(&mut below, rest, add);
        directory.children = below;
    }
}

/// Sorts every directory by name, and every directory's albums by title.
fn sort_tree(directories: &mut [Directory], albums: &[Album]) {
    directories.sort_by_key(|directory| directory.name.to_lowercase());
    for directory in directories.iter_mut() {
        directory.albums.sort_by(|&a, &b| {
            albums[a]
                .title
                .to_lowercase()
                .cmp(&albums[b].title.to_lowercase())
        });
        sort_tree(&mut directory.children, albums);
    }
}

/// Album and disc number order, with untagged files last and titles breaking
/// ties so the order is stable whatever the file system returns.
fn order(a: &Track, b: &Track) -> std::cmp::Ordering {
    a.disc
        .unwrap_or(u32::MAX)
        .cmp(&b.disc.unwrap_or(u32::MAX))
        .then(
            a.number
                .unwrap_or(u32::MAX)
                .cmp(&b.number.unwrap_or(u32::MAX)),
        )
        .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
}

/// What a scan reports while it runs.
pub enum ScanMessage {
    /// Files read so far, for the progress line.
    Progress(usize),
    /// The finished library.
    Done(Box<Library>),
    /// The scan could not read the directory at all.
    Failed(String),
}

/// Starts a scan of `root` on a thread and returns the channel it reports on.
///
/// `ignore_hidden` leaves out files and directories whose name starts with a
/// dot, which is where desktops keep their own bookkeeping and where a music
/// directory sometimes holds a stash of files nobody wants in a playlist.
pub fn spawn_scan(root: PathBuf, ignore_hidden: bool) -> Receiver<ScanMessage> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let progress = sender.clone();
        let result = read_library_with(&root, ignore_hidden, |files| {
            let _ = progress.send(ScanMessage::Progress(files));
        });
        let message = match result {
            Ok(library) => ScanMessage::Done(Box::new(library)),
            Err(error) => ScanMessage::Failed(error),
        };
        let _ = sender.send(message);
    });
    receiver
}

/// Reads every audio file under `root` into a library, blocking until done.
///
/// Hidden files are left out, which is what the app does by default.
#[cfg(test)]
pub fn read_library(root: &Path) -> Result<Library, String> {
    read_library_with(root, true, |_| {})
}

/// Reads every audio file under `root`, hidden ones included.
#[cfg(test)]
pub fn read_library_with_hidden(root: &Path) -> Result<Library, String> {
    read_library_with(root, false, |_| {})
}

/// Reads every audio file under `root`, calling `progress` as files pile up.
///
/// A file the cache already describes, and that has not changed since, is
/// taken from there instead of being read again. That is what keeps a start
/// quick: the tags of a large directory are cheap, but decoding its album art
/// is not, and the cache holds the decoded art.
fn read_library_with(
    root: &Path,
    ignore_hidden: bool,
    mut progress: impl FnMut(usize),
) -> Result<Library, String> {
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }

    let cache = crate::cache::Cache::load(root);
    let mut tracks = Vec::new();
    let mut covers = Covers::default();
    let mut seen = 0;
    let walk = WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        // The root itself is never hidden: a music directory may sit under a
        // dotted directory and still be the one the user picked.
        .filter_entry(|entry| !ignore_hidden || entry.path() == root || !is_hidden(entry.path()));
    for entry in walk.flatten() {
        if !entry.file_type().is_file() || !has_audio_extension(entry.path()) {
            continue;
        }
        seen += 1;
        if seen % PROGRESS_EVERY == 0 {
            progress(seen);
        }
        if let Some(track) = cached_track(&cache, entry.path(), &mut covers) {
            tracks.push(track);
        } else if let Some(track) = read_track(entry.path(), root, &mut covers) {
            tracks.push(track);
        }
    }
    progress(seen);

    let library = Library::from_tracks(tracks);
    // A failed write only costs the next start its speed, so it is reported
    // and not treated as a failed scan.
    if let Err(error) = crate::cache::save(&library, root) {
        eprintln!("blue-dove: {error}");
    }
    Ok(library)
}

/// The track the cache holds for `path`, when it still fits the file.
///
/// Returns `None` when there is no entry, the file changed, or the cover it
/// names has gone, which leaves the caller to read the file instead.
fn cached_track(
    cache: &Option<crate::cache::Cache>,
    path: &Path,
    covers: &mut Covers,
) -> Option<Track> {
    let cached = cache.as_ref()?.track(path)?;
    if !cached.is_fresh() {
        return None;
    }
    covers.restored(cached)
}

/// Rebuilds the library from the cache, without reading an audio file.
///
/// Answers `None` when the cache does not fit the directory any more: an entry
/// whose file changed, or whose cover went missing, makes a scan the only
/// answer, and the caller runs one.
pub fn read_cache(root: &Path) -> Option<Library> {
    let cache = crate::cache::Cache::load(root)?;
    let mut covers = Covers::default();
    let mut tracks = Vec::with_capacity(cache.all().len());
    for cached in cache.all() {
        if !cached.is_fresh() {
            return None;
        }
        tracks.push(covers.restored(cached)?);
    }
    if tracks.is_empty() {
        return None;
    }
    Some(Library::from_tracks(tracks))
}

/// Whether the last part of `path` starts with a dot.
///
/// The scan root itself is never hidden by this: a music directory may sit
/// under a dotted directory and still be the one the user picked.
fn is_hidden(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.'))
}

/// Whether the path has an extension the decoder handles.
pub fn has_audio_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| AUDIO_EXTENSIONS.contains(&extension.to_lowercase().as_str()))
}

/// Reads one file's tags. A file whose tags cannot be read is still played,
/// under its file name, when it carries no tags at all.
///
/// `root` is what a track's directory is measured from. A file outside the
/// music directory, which is what a file manager hands the app, is read with
/// its own parent as the root, so it still gets a directory name.
pub fn read_one(path: &Path) -> Option<Track> {
    let root = path.parent().unwrap_or(path);
    read_track(path, root, &mut Covers::default())
}

/// Reads one file's tags against `root`, which its directory is measured
/// from. A file whose tags cannot be read at all is still played, under its
/// file name.
fn read_track(path: &Path, root: &Path, covers: &mut Covers) -> Option<Track> {
    // A game rip carries no tags lofty knows, so it is listed under its file
    // name instead of being dropped from the library.
    let tagged = lofty::read_from_path(path).ok();
    let tag = tagged
        .as_ref()
        .and_then(|tagged| tagged.primary_tag().or_else(|| tagged.first_tag()));

    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("Untitled")
        .to_string();
    let directory = path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("Unknown Album")
        .to_string();

    let text = |key: ItemKey| {
        tag.and_then(|tag| tag.get_string(key))
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    };

    let artist = text(ItemKey::TrackArtist).unwrap_or_else(|| "Unknown Artist".to_string());
    let album_artist = text(ItemKey::AlbumArtist).unwrap_or_else(|| artist.clone());
    let album_tag = text(ItemKey::AlbumTitle);
    let tagged_album = album_tag.is_some();
    let album = album_tag.unwrap_or(directory);
    let title = text(ItemKey::TrackTitle).unwrap_or(stem);
    let year = text(ItemKey::RecordingDate)
        .or_else(|| text(ItemKey::Year))
        .and_then(|date| date.get(..4).and_then(|year| year.parse().ok()));

    let embedded = tag
        .and_then(|tag| tag.pictures().first())
        .and_then(|picture| covers.shared(picture.data()));
    let cover = embedded.or_else(|| {
        let directory = path.parent()?;
        covers.directory_cover(directory)
    });

    Some(Track {
        path: path.to_path_buf(),
        title,
        artist,
        album,
        album_artist,
        year,
        disc: tag.and_then(Accessor::disk),
        number: tag.and_then(Accessor::track),
        duration: tagged
            .as_ref()
            .map(|tagged| tagged.properties().duration())
            .unwrap_or_default(),
        cover,
        tagged_album,
        directory: path
            .parent()
            .and_then(|parent| parent.strip_prefix(root).ok())
            .map(Path::to_path_buf)
            .unwrap_or_default(),
    })
}

/// Cover art found during a scan, decoded once each.
///
/// An album whose every track embeds the same artwork decodes it once and
/// holds one copy, and a directory image is read once rather than per track.
#[derive(Default)]
struct Covers {
    /// Decoded covers by the hash of their encoded bytes.
    by_hash: HashMap<u64, Option<Arc<Cover>>>,
    /// Directory images, by directory.
    by_directory: HashMap<PathBuf, Option<Arc<Cover>>>,
    /// Covers read back from the cache, by the key they are stored under.
    by_key: HashMap<String, Option<Arc<Cover>>>,
}

impl Covers {
    /// Rebuilds a cached track, reading its cover at most once per key.
    fn restored(&mut self, cached: &crate::cache::CachedTrack) -> Option<Track> {
        // A track whose art went missing is read again, so the album keeps
        // the picture it had.
        let cover = match cached.cover_key() {
            Some(key) => Some(self.stored(key)?),
            None => None,
        };
        Some(cached.to_track(cover))
    }

    /// The cover stored under `key`, read at most once.
    ///
    /// A key whose art is gone is remembered as missing too, so a cache that
    /// lost a picture is asked for it once rather than once per track.
    fn stored(&mut self, key: &str) -> Option<Arc<Cover>> {
        if let Some(cover) = self.by_key.get(key) {
            return cover.clone();
        }
        let cover = crate::cache::read_cover(key).map(Arc::new);
        self.by_key.insert(key.to_string(), cover.clone());
        cover
    }

    /// The decoded cover for `bytes`, decoding it the first time.
    fn shared(&mut self, bytes: &[u8]) -> Option<Arc<Cover>> {
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        let key = hasher.finish();
        if let Some(cover) = self.by_hash.get(&key) {
            return cover.clone();
        }
        let cover = crate::cover::decode(bytes).map(Arc::new);
        self.by_hash.insert(key, cover.clone());
        cover
    }

    /// The cover image belonging to `directory`, when there is one.
    ///
    /// Release directories hold the artwork in one of a few ways: beside the
    /// tracks under a conventional name, in a `covers` or `scans` directory, or
    /// for a multi-disc rip in the directory above the disc directories. All three
    /// are looked for, best first.
    fn directory_cover(&mut self, directory: &Path) -> Option<Arc<Cover>> {
        if let Some(cover) = self.by_directory.get(directory) {
            return cover.clone();
        }
        let cover = art_in(directory)
            .or_else(|| art_directory_of(directory))
            // A disc directory's artwork usually sits with the other discs.
            .or_else(|| {
                let parent = directory
                    .parent()
                    .filter(|_| is_disc_directory(directory))?;
                art_in(parent).or_else(|| art_directory_of(parent))
            })
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|bytes| crate::cover::decode(&bytes))
            .map(Arc::new);
        self.by_directory
            .insert(directory.to_path_buf(), cover.clone());
        cover
    }
}

/// The front cover sitting in `directory` itself, if there is one.
///
/// A conventional name wins; failing that, an image whose name starts with
/// one of the usual words; failing that, the only image there, which is what
/// a directory holding nothing but its art looks like.
fn art_in(directory: &Path) -> Option<PathBuf> {
    for name in COVER_NAMES {
        let path = directory.join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    let images = images_in(directory);
    for prefix in COVER_PREFIXES {
        if let Some(path) = images.iter().find(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem.to_lowercase().starts_with(prefix))
        }) {
            return Some(path.clone());
        }
    }
    match images.len() {
        1 => images.first().cloned(),
        _ => None,
    }
}

/// The front cover in `directory`'s artwork directory, if it has one.
fn art_directory_of(directory: &Path) -> Option<PathBuf> {
    ART_DIRECTORIES
        .iter()
        .map(|name| directory.join(name))
        .filter(|path| path.is_dir())
        .find_map(|path| art_in(&path))
}

/// The image files directly inside `directory`, in a stable order.
fn images_in(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut images: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| {
                        IMAGE_EXTENSIONS.contains(&extension.to_lowercase().as_str())
                    })
        })
        .collect();
    images.sort();
    images
}

/// Whether `directory` holds one disc of a multi-disc release.
///
/// Its artwork is then usually in the directory above it, with the other discs.
fn is_disc_directory(directory: &Path) -> bool {
    let Some(name) = directory.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let name = name.trim().to_lowercase();
    ["disc", "cd", "dvd", "disk"].iter().any(|word| {
        name.strip_prefix(word).is_some_and(|rest| {
            let rest = rest.trim_start_matches([' ', '-', '_', '.']);
            rest.chars()
                .next()
                .is_some_and(|next| next.is_ascii_digit())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A one- or two-second mono WAV, for a directory the tests scan.
    fn wav(seconds: u32) -> Vec<u8> {
        let rate = 8000_u32;
        let samples = rate * seconds;
        let mut data = Vec::with_capacity(samples as usize * 2);
        for index in 0..samples {
            let value = if index % 2 == 0 { 1000_i16 } else { -1000 };
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
        wav
    }

    /// A directory of `count` WAV files, removed when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str, count: usize) -> Self {
            let root =
                std::env::temp_dir().join(format!("blue-dove-{name}-{}", std::process::id()));
            let album = root.join("An Album");
            std::fs::create_dir_all(&album).expect("the directory is made");
            for index in 0..count {
                let path = album.join(format!("{index:02}.wav"));
                std::fs::File::create(&path)
                    .and_then(|mut file| file.write_all(&wav(1)))
                    .expect("the file is written");
            }
            Self(root)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn hidden_files_are_left_out_by_default() {
        let directory = Scratch::new("hidden", 2);
        let album = directory.0.join("An Album");
        for name in [".hidden.wav", ".hidden-dir/inner.wav"] {
            let path = directory.0.join(name);
            std::fs::create_dir_all(path.parent().expect("a file has a parent"))
                .expect("the directory is made");
            std::fs::File::create(&path)
                .and_then(|mut file| file.write_all(&wav(1)))
                .expect("the file is written");
        }
        // A file whose name merely contains a dot is not hidden.
        let dotted = album.join("song.v2.wav");
        std::fs::File::create(&dotted)
            .and_then(|mut file| file.write_all(&wav(1)))
            .expect("the file is written");

        let library = read_library(&directory.0).expect("the directory scans");
        assert_eq!(library.tracks.len(), 3, "the two hidden files are skipped");

        let all = read_library_with_hidden(&directory.0).expect("the directory scans");
        assert_eq!(all.tracks.len(), 5, "turning the option off lists them");
    }

    #[test]
    fn a_music_directory_inside_a_hidden_one_is_still_scanned() {
        // The root the user picked is not hidden by its own name, or a music
        // directory kept under a dotted one would show nothing at all.
        let directory = Scratch::new("hidden-root", 0);
        let root = directory.0.join(".music");
        std::fs::create_dir_all(root.join("An Album")).expect("the directory is made");
        std::fs::File::create(root.join("An Album").join("song.wav"))
            .and_then(|mut file| file.write_all(&wav(1)))
            .expect("the file is written");

        let library = read_library(&root).expect("the directory scans");
        assert_eq!(library.tracks.len(), 1);
    }

    #[test]
    fn a_rescan_reads_only_what_changed() {
        let directory = Scratch::new("rescan", 12);
        let first = read_library(&directory.0).expect("the directory scans");
        assert_eq!(first.tracks.len(), 12);

        // Rewriting one file is what a retag looks like to the cache.
        let victim = first.tracks[0].path.clone();
        std::fs::File::create(&victim)
            .and_then(|mut file| file.write_all(&wav(2)))
            .expect("the file is rewritten");

        let second = read_library(&directory.0).expect("the directory scans again");
        assert_eq!(second.tracks.len(), 12);
        let reread = second
            .tracks
            .iter()
            .find(|track| track.path == victim)
            .expect("the rewritten file is listed");
        assert_eq!(
            reread.duration,
            Duration::from_secs(2),
            "the changed file was read again; the others came from the cache"
        );
    }

    #[test]
    fn the_cache_rebuilds_the_library_on_its_own() {
        let directory = Scratch::new("cache", 6);
        let scanned = read_library(&directory.0).expect("the directory scans");
        let cached = read_cache(&directory.0).expect("the cache fits the directory");

        assert_eq!(cached.tracks.len(), scanned.tracks.len());
        assert_eq!(cached.albums.len(), scanned.albums.len());
        for (from_scan, from_cache) in scanned.tracks.iter().zip(&cached.tracks) {
            assert_eq!(from_scan.path, from_cache.path);
            assert_eq!(from_scan.duration, from_cache.duration);
        }
    }

    #[test]
    fn a_directory_the_cache_never_saw_is_scanned() {
        let directory = Scratch::new("no-cache", 2);
        let elsewhere = directory.0.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).expect("the directory is made");
        // Nothing has cached this directory, so it is read the usual way.
        read_library(&directory.0).expect("the directory scans");
        assert!(read_cache(&elsewhere).is_none());
    }

    fn track(album: &str, artist: &str, number: Option<u32>, title: &str) -> Track {
        Track {
            path: PathBuf::from(format!("/music/{album}/{title}.flac")),
            title: title.to_string(),
            artist: artist.to_string(),
            album: album.to_string(),
            album_artist: artist.to_string(),
            year: Some(2007),
            disc: None,
            number,
            duration: Duration::from_secs(180),
            cover: None,
            directory: PathBuf::from(album),
            tagged_album: true,
        }
    }

    #[test]
    fn every_track_knows_the_album_it_belongs_to() {
        let mut loose = track("Loose", "Someone", None, "A File");
        loose.tagged_album = false;
        let library = Library::from_tracks(vec![
            track("Skies", "Someone", Some(1), "First"),
            loose,
            track("Skies", "Someone", Some(2), "Second"),
        ]);

        let albums = library.album_indices();
        assert_eq!(albums.len(), library.tracks.len());
        assert_eq!(
            albums[0], albums[2],
            "two tracks of one album share its index"
        );
        assert_eq!(albums[1], None, "a file with no album tag belongs to none");
        assert_eq!(
            library.albums[albums[0].expect("the first track has an album")].title,
            "Skies"
        );
    }

    #[test]
    fn groups_tracks_by_album_and_orders_them_by_number() {
        let library = Library::from_tracks(vec![
            track("Skies", "Various Artists", Some(3), "Third"),
            track("Skies", "Various Artists", Some(1), "First"),
            track("Skies", "Various Artists", Some(2), "Second"),
            track("Other", "Someone", Some(1), "Only"),
        ]);

        assert_eq!(library.albums.len(), 2);
        let skies = &library.albums[0];
        assert_eq!(skies.title, "Other");
        let skies = &library.albums[1];
        let titles: Vec<&str> = skies
            .tracks
            .iter()
            .map(|&index| library.tracks[index].title.as_str())
            .collect();
        assert_eq!(titles, ["First", "Second", "Third"]);
    }

    #[test]
    fn untagged_files_sort_after_numbered_ones() {
        let library = Library::from_tracks(vec![
            track("Skies", "Someone", None, "Untagged"),
            track("Skies", "Someone", Some(1), "Numbered"),
        ]);
        assert_eq!(library.albums[0].tracks, [1, 0]);
    }

    #[test]
    fn reports_the_album_length() {
        let library = Library::from_tracks(vec![
            track("Skies", "Someone", Some(1), "One"),
            track("Skies", "Someone", Some(2), "Two"),
        ]);
        assert_eq!(
            library.albums[0].duration(&library),
            Duration::from_secs(360)
        );
    }

    #[test]
    fn the_file_name_keeps_its_extension_only_when_asked() {
        let mut one = track("Skies", "Someone", Some(1), "One");
        one.path = PathBuf::from("/music/Skies/01 Opening Theme.flac");

        assert_eq!(one.file_name(true), "01 Opening Theme.flac");
        assert_eq!(
            one.file_name(false),
            "01 Opening Theme",
            "without the extension the name reads like a title"
        );
    }

    #[test]
    fn a_name_with_dots_keeps_the_dots_that_are_not_the_extension() {
        let mut one = track("Skies", "Someone", Some(1), "One");
        one.path = PathBuf::from("/music/Skies/2.01 Main Theme.mp3");

        assert_eq!(one.file_name(false), "2.01 Main Theme");
    }

    #[test]
    fn a_directory_holding_one_album_is_a_pass_through() {
        let mut single = track("Skies", "Someone", Some(1), "One");
        single.directory = PathBuf::from("Skies/Disc 1");
        let mut other = track("Other", "Someone", Some(1), "Two");
        other.directory = PathBuf::from("Box/Other");
        let library = Library::from_tracks(vec![single, other]);

        let skies = library
            .directories
            .iter()
            .find(|directory| directory.name == "Skies")
            .expect("the release directory");
        assert!(skies.is_pass_through(), "one album, nothing else");
        let disc = &skies.children[0];
        assert!(disc.is_pass_through(), "the leaf directory wraps its album");
    }

    #[test]
    fn a_directory_holding_two_albums_is_not_a_pass_through() {
        let mut one = track("One", "Someone", Some(1), "A");
        one.directory = PathBuf::from("Compilation/Disc 1");
        let mut two = track("Two", "Someone", Some(1), "B");
        two.directory = PathBuf::from("Compilation/Disc 2");
        let library = Library::from_tracks(vec![one, two]);

        let compilation = &library.directories[0];
        assert_eq!(compilation.name, "Compilation");
        assert!(!compilation.is_pass_through());
        assert_eq!(compilation.children.len(), 2);
        assert!(compilation.children.iter().all(Directory::is_pass_through));
    }

    #[test]
    fn loose_files_stop_a_directory_from_being_a_pass_through() {
        let mut album = track("One", "Someone", Some(1), "A");
        album.directory = PathBuf::from("Mixed");
        let mut loose = track("Mixed", "Someone", Some(1), "stray");
        loose.directory = PathBuf::from("Mixed");
        loose.tagged_album = false;
        let library = Library::from_tracks(vec![album, loose]);

        let mixed = &library.directories[0];
        assert_eq!(mixed.loose.len(), 1);
        assert!(!mixed.is_pass_through());
    }

    #[test]
    fn a_missing_directory_is_an_error() {
        let missing = std::env::temp_dir().join("blue-dove-no-such-directory");
        assert!(read_library(&missing).is_err());
    }

    #[test]
    fn an_empty_directory_is_an_empty_library() {
        let directory =
            std::env::temp_dir().join(format!("blue-dove-empty-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the temporary directory is created");
        let library = read_library(&directory).expect("an empty directory scans");
        assert!(library.is_empty());
        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
    }

    /// A directory of artwork, removed when the test ends.
    struct Art(PathBuf);

    impl Art {
        fn new(name: &str) -> Self {
            let root =
                std::env::temp_dir().join(format!("blue-dove-art-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("the directory is made");
            Self(root)
        }

        /// Puts a 1x1 PNG named `name` in `directory`, under this directory.
        fn image(&self, directory: &str, name: &str) -> PathBuf {
            let directory = self.0.join(directory);
            std::fs::create_dir_all(&directory).expect("the directory is made");
            let path = directory.join(name);
            let file = std::fs::File::create(&path).expect("the image is created");
            let mut encoder = png::Encoder::new(file, 1, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder
                .write_header()
                .expect("the image has a header")
                .write_image_data(&[0, 0, 0, 255])
                .expect("the image has pixels");
            path
        }
    }

    impl Drop for Art {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_cover_a_directory_names_is_the_one_used() {
        let art = Art::new("named");
        let directory = art.0.join("Album");
        std::fs::create_dir_all(&directory).expect("the directory is made");
        art.image("Album", "back.png");
        art.image("Album", "Cover1.jpg");

        assert_eq!(
            art_in(&directory),
            Some(directory.join("Cover1.jpg")),
            "a name that says it is the cover beats one that does not"
        );
    }

    #[test]
    fn a_directory_of_images_without_a_cover_is_not_guessed_at() {
        let art = Art::new("unclear");
        art.image("Album", "back.png");
        art.image("Album", "inside.png");

        assert_eq!(
            art_in(&art.0.join("Album")),
            None,
            "two images and no cover among them leaves the album without art"
        );
    }

    #[test]
    fn art_kept_in_its_own_directory_is_found() {
        let art = Art::new("covers");
        let cover = art.image("Album/covers", "front.jpg");

        assert_eq!(art_directory_of(&art.0.join("Album")), Some(cover));
    }

    #[test]
    fn a_disc_directory_takes_the_art_from_above_it() {
        let art = Art::new("disc");
        let root = art.0.join("Release");
        std::fs::create_dir_all(root.join("Disc 1")).expect("the disc directory is made");
        art.image("Release", "cover.jpg");

        let mut covers = Covers::default();
        let cover = covers
            .directory_cover(&root.join("Disc 1"))
            .expect("the disc directory finds the set's art");

        assert_eq!((cover.width, cover.height), (1, 1));
    }

    #[test]
    fn only_a_disc_directory_names_a_disc() {
        for name in ["Disc 1", "disc-2", "CD3", "cd_4", "DVD 1"] {
            assert!(is_disc_directory(Path::new(name)), "{name} is a disc");
        }
        for name in ["Discography", "CDs", "Disco", "Music"] {
            assert!(!is_disc_directory(Path::new(name)), "{name} is not a disc");
        }
    }

    /// The mimetypes shared-mime-info gives each extension the player reads.
    ///
    /// An empty list is an extension no desktop knows: the game rip formats,
    /// CAF and MP1. MP4 is left out too, since its mimetype also names the
    /// video files the player does not open.
    const MIMETYPES: &[(&str, &[&str])] = &[
        ("aac", &["audio/aac"]),
        ("adx", &[]),
        ("aif", &["audio/x-aiff"]),
        ("aiff", &["audio/x-aiff"]),
        ("amr", &["audio/AMR"]),
        ("ape", &["audio/x-ape"]),
        ("bcstm", &[]),
        ("bfstm", &[]),
        ("brstm", &[]),
        ("caf", &[]),
        ("dsp", &["audio/x-dsp"]),
        ("flac", &["audio/flac"]),
        ("m4a", &["audio/mp4"]),
        ("m4b", &["audio/x-m4b"]),
        ("mka", &["audio/matroska"]),
        ("mp+", &["audio/x-musepack"]),
        ("mp1", &[]),
        ("mp2", &["audio/mp2"]),
        ("mp3", &["audio/mpeg"]),
        ("mp4", &[]),
        ("mpc", &["audio/x-musepack"]),
        ("mpp", &["audio/x-musepack"]),
        ("oga", OGG),
        ("ogg", OGG),
        ("opus", &["audio/ogg", "audio/x-opus+ogg"]),
        ("shn", &["application/x-shorten"]),
        ("swag", &[]),
        ("tta", &["audio/x-tta"]),
        ("vag", &[]),
        ("vig", &[]),
        ("wav", &["audio/vnd.wave"]),
        ("wv", &["audio/x-wavpack"]),
    ];

    /// What an Ogg file may hold: Vorbis, FLAC or Speex, in a stream an
    /// older database calls `audio/x-vorbis+ogg` and a newer one
    /// `audio/vorbis`.
    const OGG: &[&str] = &[
        "audio/ogg",
        "audio/vorbis",
        "audio/x-vorbis+ogg",
        "audio/x-flac+ogg",
        "audio/x-speex+ogg",
    ];

    /// A file manager offers the player for every format it reads, and the
    /// desktop entry claims no mimetype the player cannot open.
    #[test]
    fn the_desktop_entry_claims_the_formats_the_player_reads() {
        let entry = include_str!("../packaging/linux/app.desktop");
        let line = entry
            .lines()
            .find_map(|line| line.strip_prefix("MimeType="))
            .expect("the desktop entry names the mimetypes it opens");
        let claimed: Vec<&str> = line.split(';').filter(|mime| !mime.is_empty()).collect();

        let mut listed: Vec<&str> = MIMETYPES.iter().map(|(extension, _)| *extension).collect();
        listed.sort_unstable();
        let mut read = AUDIO_EXTENSIONS.to_vec();
        read.sort_unstable();
        assert_eq!(listed, read, "every format the player reads belongs here");

        for (extension, mimetypes) in MIMETYPES {
            for mimetype in *mimetypes {
                assert!(
                    claimed.contains(mimetype),
                    "{extension} opens as {mimetype}, which the entry leaves out"
                );
            }
        }
        for mimetype in &claimed {
            assert!(
                MIMETYPES.iter().any(|(_, known)| known.contains(mimetype)),
                "{mimetype} is claimed but no format the player reads has it"
            );
        }
    }

    /// The entry hands the player the file that was opened, so a file manager
    /// that launches it plays that file rather than showing an empty window.
    #[test]
    fn the_desktop_entry_passes_the_opened_file() {
        let entry = include_str!("../packaging/linux/app.desktop");
        let exec = entry
            .lines()
            .find_map(|line| line.strip_prefix("Exec="))
            .expect("the desktop entry names the command that runs");
        assert!(
            exec.contains("%F"),
            "Exec={exec} names no file, so a file manager opens an empty player"
        );
    }
}
