//! Choosing a music directory through the desktop's own file dialog.
//!
//! `rfd` asks the XDG file-chooser portal, which gives the dialog the user's
//! desktop draws, and falls back to `zenity` when no portal answers. Both
//! browse the file system themselves, so a symlinked directory is followed the
//! way every other program follows one.
//!
//! A dialog stays open until the user answers, so it runs on a thread of its
//! own and reports over a channel; the window keeps drawing meanwhile.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver};
use std::thread;

/// What to say when no dialog could be shown at all.
const MISSING: &str = "No file dialog could open: install zenity, or an \
     xdg-desktop-portal with a backend such as xdg-desktop-portal-gtk";

/// Starts the dialog on a thread and returns the channel it answers on.
pub fn spawn(start: PathBuf) -> Receiver<Option<PathBuf>> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let _ = sender.send(pick_directory(&start));
    });
    receiver
}

/// Asks for a directory, blocking until the dialog closes.
///
/// `None` means the user cancelled, or that no dialog backend answered:
/// `rfd` reports both the same way. [`explain`] tells the second case from
/// the first when it can.
pub fn pick_directory(start: &Path) -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title("Choose a music directory");
    if start.is_dir() {
        dialog = dialog.set_directory(start);
    }
    dialog.pick_folder()
}

/// The message to show when [`pick_directory`] answered nothing.
///
/// A cancel and a missing backend look alike from the return value, so this
/// speaks only when the portal is known to be absent *and* zenity is not
/// installed, which is the case the user can act on.
pub fn explain() -> Option<String> {
    if cfg!(not(target_os = "linux")) {
        return None;
    }
    if portal_available() || zenity_installed() {
        return None;
    }
    Some(MISSING.to_string())
}

/// Whether zenity is on `PATH`, the fallback `rfd` uses.
fn zenity_installed() -> bool {
    std::env::var_os("PATH").is_some_and(|path| zenity_on(&path))
}

/// Whether `path` holds an executable named zenity.
fn zenity_on(path: &OsStr) -> bool {
    std::env::split_paths(path).any(|dir| dir.join("zenity").is_file())
}

/// Whether a portal offering a file chooser is on the session bus.
///
/// Anything inconclusive counts as available, so a working system never sees
/// the message. The bus is asked once; the answer is kept.
fn portal_available() -> bool {
    static ANSWER: OnceLock<bool> = OnceLock::new();
    *ANSWER.get_or_init(probe_portal)
}

/// Asks the portal object on the session bus whether it exports the file
/// chooser interface.
fn probe_portal() -> bool {
    let Ok(connection) = zbus::blocking::Connection::session() else {
        return false;
    };
    let reply = connection.call_method(
        Some("org.freedesktop.portal.Desktop"),
        "/org/freedesktop/portal/desktop",
        Some("org.freedesktop.DBus.Introspectable"),
        "Introspect",
        &(),
    );
    match reply {
        Ok(message) => match message.body().deserialize::<String>() {
            Ok(xml) => xml.contains("org.freedesktop.portal.FileChooser"),
            // A reply that cannot be read says nothing about the backend.
            Err(_) => true,
        },
        // A timeout or a dropped connection is not proof that it is absent.
        Err(error) => !is_absent(&error),
    }
}

/// Whether a D-Bus error means the service, object or interface is not there.
fn is_absent(error: &zbus::Error) -> bool {
    use zbus::fdo::Error as Fdo;
    let zbus::Error::FDO(fdo) = error else {
        return false;
    };
    matches!(
        &**fdo,
        Fdo::ServiceUnknown(_)
            | Fdo::NameHasNoOwner(_)
            | Fdo::UnknownObject(_)
            | Fdo::UnknownInterface(_)
            | Fdo::UnknownMethod(_)
            | Fdo::UnknownProperty(_)
            | Fdo::InvalidArgs(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zenity_is_found_in_a_path_entry() {
        let directory =
            std::env::temp_dir().join(format!("blue-dove-zenity-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("the temporary directory is created");
        let path = std::env::join_paths([directory.as_os_str()]).expect("a single entry joins");

        assert!(!zenity_on(&path), "an empty directory holds no zenity");

        std::fs::write(directory.join("zenity"), b"#!/bin/sh\n").expect("the stand-in is written");
        let path = std::env::join_paths([directory.as_os_str()]).expect("a single entry joins");
        let found = zenity_on(&path);

        std::fs::remove_dir_all(&directory).expect("the temporary directory is removed");
        assert!(found);
    }

    #[test]
    fn the_message_names_what_to_install() {
        assert!(MISSING.contains("zenity"));
        assert!(MISSING.contains("xdg-desktop-portal"));
    }
}
