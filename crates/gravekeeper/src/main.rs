#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod cli;
mod dock;
mod elevate;
mod gui;
mod headless;
mod icon;
mod os_settings;
mod service;
mod shell;
mod tray;
mod ui;

use std::process::ExitCode;
use std::sync::Mutex;
use std::time::Instant;

use gravekeeper_core::logfile::RotatingFile;
use gravekeeper_core::paths::Paths;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::cli::Mode;

fn main() -> ExitCode {
    let process_started = Instant::now();
    let mode = match cli::parse(std::env::args().skip(1)) {
        Ok(mode) => mode,
        Err(message) => {
            eprintln!("{message}\n\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    match mode {
        Mode::Help => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Mode::Version => {
            println!("gravekeeper {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Mode::Service(command) => return service::run(command),
        // Before logging starts: the helper must not write to the user's data directory.
        Mode::Elevated { request, report } => {
            return elevate::helper(&request, report.as_deref());
        }
        Mode::Gui { .. } | Mode::Headless => {}
    }

    let paths = Paths::from_env();
    init_logging(paths.as_ref().ok());
    let paths = match paths {
        Ok(paths) => paths,
        Err(err) => {
            tracing::error!(%err, "cannot determine where to store data");
            return ExitCode::FAILURE;
        }
    };
    match mode {
        Mode::Headless => headless::run(paths),
        Mode::Gui { minimized } => gui::run(paths, minimized, process_started),
        Mode::Help | Mode::Version | Mode::Service(_) | Mode::Elevated { .. } => {
            unreachable!("handled above")
        }
    }
}

/// Logs to stderr and, when the data directory is known, to `<data>/logs/gravekeeper.log`
/// (rotated at 10 MiB, 5 old files kept).
fn init_logging(paths: Option<&Paths>) {
    let filter =
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,zbus=error"));
    let file = paths.map(|p| {
        let file = RotatingFile::new(p.data_dir.join("logs/gravekeeper.log"), 10 << 20, 5);
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(Mutex::new(file))
            .with_filter(filter())
    });
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter()))
        .with(file)
        .init();
}
