//! A fallback font for titles written in Japanese, Chinese or Korean.
//!
//! egui's bundled fonts cover Latin only, so a library tagged in Japanese
//! would draw boxes. A system font that carries those glyphs is loaded as a
//! fallback, which costs nothing when none is installed.
//!
//! Which font is installed differs from distribution to distribution and
//! from user to user, so the font directories are searched rather than a
//! list of paths being trusted: a hardcoded list misses the fonts that are
//! actually there, and every title shows as boxes.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui::{self, FontData, FontDefinitions, FontFamily};

/// File name fragments of fonts that carry CJK glyphs, best first.
///
/// A full CJK font is preferred to a Japanese-only one, since it also draws
/// Chinese and Korean titles, and both are preferred to a fallback face.
const PATTERNS: &[&str] = &[
    "notosanscjk",
    "notoserifcjk",
    "sourcehansans",
    "sourcehanserif",
    "notosansjp",
    "notoserifjp",
    "droidsansfallbackfull",
    "droidsansfallback",
    "droidsansjapanese",
    "ipaexg",
    "ipaexm",
    "ipag",
    "ipam",
    "takaogothic",
    "takaomincho",
    "takaopgothic",
    "takaopmincho",
    "vlgothic",
    "vl-gothic",
    "sarasa",
    "mplus",
    "wenquanyi",
    "wqy",
    "msyh",
    "yugoth",
    "meiryo",
    "msgothic",
    "simsun",
    "simhei",
    "pingfang",
    "hiraginosans",
    "arialuni",
];

/// Font directories, as the systems that matter lay them out.
const DIRECTORIES: &[&str] = &[
    "/usr/share/fonts",
    "/usr/local/share/fonts",
    "/usr/share/X11/fonts",
    "/System/Library/Fonts",
    "/Library/Fonts",
    "C:\\Windows\\Fonts",
];

/// Extensions a font file can carry.
const EXTENSIONS: &[&str] = &["ttf", "otf", "ttc", "otc"];

/// Adds the best CJK font found as a fallback for both families.
pub fn install(ctx: &egui::Context) {
    let Some(path) = find() else {
        return;
    };
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };

    let mut fonts = FontDefinitions::default();
    fonts
        .font_data
        .insert("cjk".to_owned(), Arc::new(FontData::from_owned(bytes)));
    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .push("cjk".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// The best CJK font installed, if there is one.
fn find() -> Option<PathBuf> {
    let mut files = Vec::new();
    for directory in directories() {
        collect(&directory, 0, &mut files);
    }
    pick(&files)
}

/// The font directories of the system and of the user.
fn directories() -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = DIRECTORIES.iter().map(PathBuf::from).collect();
    if let Some(data) = dirs::data_dir() {
        directories.push(data.join("fonts"));
    }
    if let Some(home) = dirs::home_dir() {
        directories.push(home.join(".fonts"));
        directories.push(home.join(".local").join("share").join("fonts"));
    }
    if let Some(windows) = std::env::var_os("WINDIR") {
        directories.push(PathBuf::from(windows).join("Fonts"));
    }
    directories
}

/// Collects the font files under `directory`, a few levels deep.
fn collect(directory: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, depth + 1, files);
        } else if is_font(&path) {
            files.push(path);
        }
    }
}

/// Whether `path` names a font file.
fn is_font(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            let extension = extension.to_ascii_lowercase();
            EXTENSIONS.contains(&extension.as_str())
        })
}

/// The best of `files`, by the order [`PATTERNS`] gives.
///
/// A pattern is searched for in the whole path, so a font in a directory named
/// after its family is found too, and the first pattern that matches wins.
fn pick(files: &[PathBuf]) -> Option<PathBuf> {
    PATTERNS.iter().find_map(|pattern| {
        files.iter().find_map(|path| {
            let name = squashed(&path.to_string_lossy());
            name.contains(pattern).then(|| path.clone())
        })
    })
}

/// The letters and digits of `text`, in lower case.
///
/// Font files spell a family `Noto_Sans_CJK`, `Noto Sans CJK` or
/// `NotoSansCJK` depending on who packaged them, so the separators are
/// dropped before a name is compared with a pattern.
fn squashed(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|letter| letter.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn a_full_cjk_font_is_preferred_to_a_japanese_one() {
        let files = paths(&[
            "/usr/share/fonts/droid/DroidSansJapanese.ttf",
            "/usr/share/fonts/noto/NotoSansCJK-Regular.ttc",
        ]);
        assert_eq!(
            pick(&files),
            Some(PathBuf::from(
                "/usr/share/fonts/noto/NotoSansCJK-Regular.ttc"
            ))
        );
    }

    #[test]
    fn a_font_the_old_list_missed_is_found() {
        // The face a stock Arch or Debian install carries, which a list of
        // fixed paths did not name.
        let files = paths(&[
            "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/droid/DroidSansFallback.ttf",
        ]);
        assert_eq!(
            pick(&files),
            Some(PathBuf::from(
                "/usr/share/fonts/droid/DroidSansFallback.ttf"
            ))
        );
    }

    #[test]
    fn the_directory_name_counts_as_well_as_the_file_name() {
        let files = paths(&["/usr/share/fonts/ipa/ipa-g.ttf"]);
        assert_eq!(
            pick(&files),
            Some(PathBuf::from("/usr/share/fonts/ipa/ipa-g.ttf"))
        );
    }

    #[test]
    fn a_name_split_by_separators_still_matches() {
        // Packagers spell a family three ways; all of them have to be found.
        for name in [
            "/usr/share/fonts/noto/Noto_Sans_CJK-Regular.ttc",
            "/usr/share/fonts/noto/Noto Sans CJK Regular.otf",
            "/usr/share/fonts/noto/NotoSansCJK-Regular.ttc",
        ] {
            let files = paths(&[name]);
            assert_eq!(pick(&files), Some(PathBuf::from(name)), "{name}");
        }
    }

    #[test]
    fn a_latin_only_system_gets_no_fallback() {
        let files = paths(&[
            "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        ]);
        assert_eq!(pick(&files), None, "no font here draws Japanese");
    }

    #[test]
    fn only_font_files_are_collected() {
        assert!(is_font(Path::new("/x/NotoSansCJK.ttc")));
        assert!(is_font(Path::new("/x/DroidSansJapanese.TTF")));
        assert!(!is_font(Path::new("/x/fonts.dir")));
        assert!(!is_font(Path::new("/x/README")));
    }
}
