#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod autostart;
mod shell;
mod tray;
mod ui;

use std::process::ExitCode;
use std::sync::{Arc, mpsc};
use std::time::Instant;

use eframe::egui;
use gravekeeper_core::paths::Paths;
use gravekeeper_core::{CoreHandle, StartError};
use std::sync::Mutex;

use gravekeeper_core::logfile::RotatingFile;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tray_icon::TrayIcon;

use crate::shell::{CloseBehavior, Shell, TrayAction};

const APP_ID: &str = "gravekeeper";

struct Options {
    start_minimized: bool,
    close: CloseBehavior,
}

impl Options {
    fn parse() -> Result<Self, String> {
        let mut start_minimized = false;
        for arg in std::env::args().skip(1) {
            match arg.as_str() {
                "--minimized" => start_minimized = true,
                other => {
                    return Err(format!(
                        "unknown argument: {other}\nusage: gravekeeper [--minimized]"
                    ));
                }
            }
        }
        Ok(Self {
            start_minimized,
            close: CloseBehavior::from_env(),
        })
    }
}

fn main() -> ExitCode {
    let process_started = Instant::now();
    let paths = Paths::from_env();
    init_logging(paths.as_ref().ok());

    let mut options = match Options::parse() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };

    let paths = match paths {
        Ok(paths) => paths,
        Err(err) => {
            tracing::error!(%err, "cannot determine where to store data");
            return ExitCode::FAILURE;
        }
    };
    let ctx = egui::Context::default();
    let repaint_ctx = ctx.clone();
    let core = match CoreHandle::start(paths, Arc::new(move || repaint_ctx.request_repaint())) {
        Ok(core) => core,
        Err(StartError::AlreadyRunning) => {
            tracing::warn!("another instance is already running");
            // No app id: eframe derives its storage location from it, and the main window's
            // saved size must not be applied here.
            let _ = eframe::run_native(
                "Gravekeeper notice",
                eframe::NativeOptions {
                    viewport: egui::ViewportBuilder::default()
                        .with_title("Gravekeeper")
                        .with_inner_size([380.0, 120.0]),
                    renderer: renderer(),
                    persist_window: false,
                    ..Default::default()
                },
                Box::new(|_| Ok(Box::new(ui::AlreadyRunning))),
            );
            return ExitCode::from(3);
        }
        Err(err) => {
            tracing::error!(%err, "failed to start engine");
            return ExitCode::FAILURE;
        }
    };
    options.start_minimized |= core.snapshot().config.general.start_minimized;

    let (wake_tx, wake_rx) = mpsc::channel();
    let shell = Arc::new(Shell::new(ctx, wake_tx));

    // The ksni tray runs on its own thread, so on Linux it can exist before (and between)
    // windows. Elsewhere it needs the running event loop, so it is created with the window.
    let mut tray: Option<TrayIcon> = None;
    if cfg!(target_os = "linux") {
        tray = create_tray(&shell, &mut options);
    }

    // Without a tray, a window is the only way to reach the application.
    let mut show_window = !options.start_minimized
        || options.close == CloseBehavior::Hide
        || options.close == CloseBehavior::Quit;
    let result = loop {
        if show_window
            && let Err(err) = run_window(&options, &shell, &core, &mut tray, process_started)
        {
            break Err(err);
        }
        if shell.quit_requested() || options.close != CloseBehavior::Destroy {
            break Ok(());
        }
        match wake_rx.recv() {
            Ok(TrayAction::Show) => show_window = true,
            Ok(TrayAction::Quit) | Err(_) => break Ok(()),
        }
    };

    core.shutdown();
    drop(tray);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(%err, "GUI failed");
            ExitCode::FAILURE
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

fn create_tray(shell: &Arc<Shell>, options: &mut Options) -> Option<TrayIcon> {
    match tray::create(shell) {
        Ok(tray) => Some(tray),
        Err(err) => {
            tracing::warn!(%err, "system tray unavailable; closing the window will quit");
            options.close = CloseBehavior::Quit;
            None
        }
    }
}

fn run_window(
    options: &Options,
    shell: &Arc<Shell>,
    core: &CoreHandle,
    tray: &mut Option<TrayIcon>,
    process_started: Instant,
) -> eframe::Result {
    // In Destroy mode a minimized start simply has no window yet, so only Hide starts hidden.
    let visible = !(options.start_minimized && options.close == CloseBehavior::Hide);
    let icon = egui::IconData {
        rgba: tray::icon_rgba(64),
        width: 64,
        height: 64,
    };
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Gravekeeper")
            .with_app_id(APP_ID)
            .with_inner_size([960.0, 640.0])
            .with_min_inner_size([640.0, 400.0])
            .with_icon(icon)
            .with_visible(visible),
        renderer: renderer(),
        ..Default::default()
    };

    shell.set_window_open(true);
    let mut close = options.close;
    let result = eframe::run_native_ext(
        "Gravekeeper",
        native_options,
        Some(shell.ctx().clone()),
        Box::new(|cc| {
            if tray.is_none() && close != CloseBehavior::Quit {
                match tray::create(shell) {
                    Ok(created) => *tray = Some(created),
                    Err(err) => {
                        tracing::warn!(%err, "system tray unavailable; closing the window will quit");
                        close = CloseBehavior::Quit;
                        // Without a tray a hidden window could never be shown again.
                        cc.egui_ctx
                            .send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    }
                }
            }
            Ok(Box::new(ui::GravekeeperApp::new(
                cc,
                Arc::clone(shell),
                core,
                close,
                process_started,
            )))
        }),
    );
    shell.set_window_open(false);
    result
}

fn renderer() -> eframe::Renderer {
    #[cfg(feature = "glow")]
    return eframe::Renderer::Glow;
    #[cfg(not(feature = "glow"))]
    return eframe::Renderer::Wgpu;
}
