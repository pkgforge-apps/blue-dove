//! A music player that reads the tags in a directory and plays it.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cache;
mod cli;
mod config;
mod cover;
mod decode;
mod dialog;
mod error;
mod fonts;
mod format;
mod game;
mod instance;
mod library;
mod mpris;
mod opus;
mod player;
mod playlist;
mod theme;
mod ui;
mod waveform;

use app::App;
use eframe::egui;

/// Window and taskbar icon, generated from `assets/icon.svg`.
const ICON_PNG: &[u8] = include_bytes!("../assets/icons/256.png");

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let smoke = args.iter().any(|arg| arg == "--smoke");
    let cli = match cli::parse(args) {
        cli::Parsed::Run(cli) => cli,
        cli::Parsed::Help => {
            println!("{}", cli::USAGE);
            return Ok(());
        }
        cli::Parsed::Version => {
            println!("{} {}", App::TITLE, env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        cli::Parsed::Error(message) => {
            eprintln!("blue-dove: {message}\n\n{}", cli::USAGE);
            std::process::exit(2);
        }
    };

    // Paths win over a transport flag: "blue-dove --play song.flac" means
    // play that song, and a path that opens the window plays anyway.
    let request = if !cli.paths.is_empty() {
        Some(instance::Request::Open(cli.paths.clone()))
    } else {
        cli.action.map(instance::Request::Action)
    };

    // A command belongs to the window that is already running. Only a path
    // with no window to take it opens one; a bare command has nothing to do.
    if !smoke && let Some(request) = &request {
        if instance::send(request) {
            return Ok(());
        }
        if cli.paths.is_empty() {
            eprintln!("blue-dove: no player is running");
            std::process::exit(1);
        }
    }

    // A smoke run must draw its frames on every machine CI gives it, so it
    // neither takes the lock nor answers another window's.
    let (lock, listener) = if smoke {
        (None, None)
    } else {
        match instance::claim() {
            // A window appeared while the request above was being sent.
            instance::Launch::AlreadyRunning => {
                let request = request.unwrap_or(instance::Request::Show);
                if !instance::send(&request) {
                    eprintln!("blue-dove: another window is running and did not answer");
                }
                return Ok(());
            }
            instance::Launch::Owner { lock, listener } => (Some(lock), listener),
        }
    };

    let icon =
        eframe::icon_data::from_png_bytes(ICON_PNG).expect("the bundled icon is a valid PNG");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(App::TITLE)
            .with_app_id(App::ID)
            .with_inner_size([1180.0, 760.0])
            // The floor is the compact layout's own size, so the window can
            // be shrunk into the popup and no further.
            .with_min_inner_size(app::COMPACT_SIZE)
            // The window is drawn translucent so the desktop shows through
            // it; a compositor blends it, and without one it looks solid.
            .with_transparent(true)
            .with_icon(icon),
        ..Default::default()
    };
    let paths = cli.paths;
    let result = eframe::run_native(
        App::TITLE,
        options,
        Box::new(move |cc| {
            fonts::install(&cc.egui_ctx);
            let mut app = App::new(&cc.egui_ctx, smoke, paths);
            if let Some(listener) = listener {
                app.watch_requests(listener, &cc.egui_ctx);
            }
            theme::apply(&cc.egui_ctx, app.theme, app.opacity());
            Ok(Box::new(app))
        }),
    );
    // The lock is held for as long as the window lives; the event loop has
    // returned, so this is where it goes.
    drop(lock);
    result
}
