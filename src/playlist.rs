//! Playlists: the queue of tracks the player works through.
//!
//! A playlist is a name and an ordered list of track indices into the
//! library. Several are open at once, shown as tabs, and each keeps its own
//! position so switching tabs resumes where that playlist was left.

/// One playlist: a name and the tracks in it.
#[derive(Clone, Debug, Default)]
pub struct Playlist {
    /// Name shown on the tab.
    pub name: String,
    /// Track indices into [`crate::library::Library::tracks`], in playing order.
    pub tracks: Vec<usize>,
    /// Index into `tracks` of the track the playlist would resume at.
    pub cursor: Option<usize>,
}

impl Playlist {
    /// A playlist with a name and no tracks.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }

    /// Appends tracks, keeping the order they are given in.
    ///
    /// A track already in the playlist is not added twice, so adding a
    /// directory twice does not double every album.
    pub fn add(&mut self, tracks: impl IntoIterator<Item = usize>) {
        for index in tracks {
            if !self.tracks.contains(&index) {
                self.tracks.push(index);
            }
        }
    }

    /// Replaces the tracks with `tracks`.
    pub fn set(&mut self, tracks: impl IntoIterator<Item = usize>) {
        self.tracks = tracks.into_iter().collect();
        self.cursor = self.cursor.filter(|&cursor| cursor < self.tracks.len());
    }

    /// Removes every track, leaving the name.
    #[cfg(test)]
    pub fn clear(&mut self) {
        self.tracks.clear();
        self.cursor = None;
    }
}

/// A shuffle of `tracks`, drawn from `seed`.
///
/// Random Order walks this one order rather than picking a track at each
/// step, so the whole list is heard before any track comes back, and going
/// back and then forward again plays the same track both times.
pub fn shuffled(tracks: &[usize], seed: u64) -> Vec<usize> {
    let mut order = tracks.to_vec();
    let mut state = seed | 1;
    for index in (1..order.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        order.swap(index, (state % (index as u64 + 1)) as usize);
    }
    order
}

/// The track after or before `current` in `tracks`, wrapping at either end.
///
/// A track that is not in the list has no neighbour.
pub fn neighbour(tracks: &[usize], current: usize, forward: bool) -> Option<usize> {
    let position = tracks.iter().position(|&index| index == current)?;
    let next = if forward {
        (position + 1) % tracks.len()
    } else {
        position.checked_sub(1).unwrap_or(tracks.len() - 1)
    };
    tracks.get(next).copied()
}

/// Every open playlist, and which one is shown.
pub struct Playlists {
    /// The open playlists, at least one.
    pub items: Vec<Playlist>,
    /// Index of the playlist the window shows.
    pub active: usize,
}

impl Default for Playlists {
    fn default() -> Self {
        Self {
            items: vec![Playlist::new("Default")],
            active: 0,
        }
    }
}

impl Playlists {
    /// The playlist the window shows.
    pub fn current(&self) -> &Playlist {
        &self.items[self.active.min(self.items.len() - 1)]
    }

    /// The playlist the window shows, to change.
    pub fn current_mut(&mut self) -> &mut Playlist {
        let index = self.active.min(self.items.len() - 1);
        &mut self.items[index]
    }

    /// Opens a new empty playlist and switches to it.
    ///
    /// The name is numbered after the playlists already open, so the tab is
    /// never blank.
    pub fn add(&mut self) {
        let name = format!("Playlist {}", self.items.len() + 1);
        self.items.push(Playlist::new(name));
        self.active = self.items.len() - 1;
    }

    /// Closes the playlist at `index` and picks a neighbour to show.
    ///
    /// The last playlist is never closed: the window always shows one.
    pub fn close(&mut self, index: usize) {
        if self.items.len() <= 1 || index >= self.items.len() {
            return;
        }
        self.items.remove(index);
        if self.active >= self.items.len() {
            self.active = self.items.len() - 1;
        } else if index < self.active {
            self.active -= 1;
        }
    }

    /// Whether the playlist at `index` is the one shown.
    pub fn is_active(&self, index: usize) -> bool {
        self.active.min(self.items.len() - 1) == index
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_a_directory_twice_does_not_duplicate_tracks() {
        let mut playlist = Playlist::new("Test");
        playlist.add([1, 2, 3]);
        playlist.add([3, 4]);
        assert_eq!(playlist.tracks, [1, 2, 3, 4]);
    }

    #[test]
    fn the_cursor_wraps_at_both_ends() {
        let mut playlist = Playlist::new("Test");
        playlist.add([4, 7, 9]);
        assert_eq!(neighbour(&playlist.tracks, 4, true), Some(7));
        assert_eq!(neighbour(&playlist.tracks, 9, true), Some(4));
        assert_eq!(neighbour(&playlist.tracks, 4, false), Some(9));
        assert_eq!(neighbour(&playlist.tracks, 7, false), Some(4));
        assert_eq!(neighbour(&playlist.tracks, 99, true), None);
    }

    #[test]
    fn a_shuffle_holds_every_track_once() {
        let tracks = [10, 20, 30, 40, 50];
        let order = shuffled(&tracks, 12345);
        let mut seen = order.clone();
        seen.sort_unstable();
        assert_eq!(
            seen, tracks,
            "a shuffle is the same tracks in another order"
        );
        assert_ne!(order, tracks, "and not simply the order they came in");
    }

    #[test]
    fn the_same_seed_draws_the_same_shuffle() {
        let tracks = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(shuffled(&tracks, 99), shuffled(&tracks, 99));
        assert_ne!(
            shuffled(&tracks, 99),
            shuffled(&tracks, 100),
            "another seed draws another order"
        );
    }

    #[test]
    fn a_short_list_is_shuffled_without_falling_over() {
        assert!(shuffled(&[], 7).is_empty());
        assert_eq!(shuffled(&[7], 7), [7]);
        assert_eq!(shuffled(&[7, 8], 7).len(), 2);
    }

    #[test]
    fn a_new_playlist_is_named_after_the_open_ones() {
        let mut playlists = Playlists::default();
        assert_eq!(playlists.current().name, "Default");
        playlists.add();
        assert_eq!(playlists.current().name, "Playlist 2");
        assert_eq!(playlists.active, 1);
    }

    #[test]
    fn the_last_playlist_cannot_be_closed() {
        let mut playlists = Playlists::default();
        playlists.close(0);
        assert_eq!(playlists.items.len(), 1);
    }

    #[test]
    fn closing_a_playlist_keeps_a_neighbour_active() {
        let mut playlists = Playlists::default();
        playlists.add();
        playlists.add();
        assert_eq!(playlists.active, 2);
        playlists.close(2);
        assert_eq!(playlists.active, 1);
        assert_eq!(playlists.current().name, "Playlist 2");
        playlists.close(0);
        assert_eq!(playlists.current().name, "Playlist 2");
    }
}
