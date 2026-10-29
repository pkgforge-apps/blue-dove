//! Adding the path a failure happened on to its message.
//!
//! Every decoder here reports a failure as text, and the path is what makes
//! one readable: "no such file" alone does not say which file. The message is
//! `path: error`, which is the form every caller already shows.

use std::fmt::Display;
use std::path::Path;

/// Rewrites a failure as `path: error`.
pub trait At<T> {
    /// Names the path the failure happened on.
    fn at(self, path: &Path) -> Result<T, String>;
}

impl<T, E: Display> At<T> for Result<T, E> {
    fn at(self, path: &Path) -> Result<T, String> {
        self.map_err(|error| format!("{}: {error}", path.display()))
    }
}
