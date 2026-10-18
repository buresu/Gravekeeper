//! The desktop application: window, tray, and the engine running alongside.

use std::process::ExitCode;
use std::sync::{Arc, mpsc};
use std::time::Instant;

use eframe::egui;
use gravekeeper_core::paths::Paths;
use gravekeeper_core::{CoreHandle, StartError};
use tray_icon::TrayIcon;

use crate::shell::{CloseBehavior, Shell, TrayAction};
use crate::{tray, ui};

const APP_ID: &str = "gravekeeper";

struct Options {
    start_minimized: bool,
    close: CloseBehavior,
}

/// Runs the GUI (window and tray) with the engine in the same process.
pub fn run(paths: Paths, start_minimized: bool, process_started: Instant) -> ExitCode {
    let mut options = Options {
        start_minimized,
        close: CloseBehavior::from_env(),
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
        rgba: crate::icon::rgba(128),
        width: 128,
        height: 128,
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
