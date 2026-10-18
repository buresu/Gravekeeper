#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod shell;
mod tray;
mod ui;

use std::process::ExitCode;
use std::sync::{Arc, mpsc};
use std::time::Instant;

use eframe::egui;
use gravekeeper_core::CoreHandle;
use tracing_subscriber::EnvFilter;
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
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,zbus=error")),
        )
        .init();

    let mut options = match Options::parse() {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };

    let ctx = egui::Context::default();
    let repaint_ctx = ctx.clone();
    let core = match CoreHandle::start(Arc::new(move || repaint_ctx.request_repaint())) {
        Ok(core) => core,
        Err(err) => {
            tracing::error!(%err, "failed to start engine");
            return ExitCode::FAILURE;
        }
    };

    let (wake_tx, wake_rx) = mpsc::channel();
    let shell = Arc::new(Shell::new(ctx, wake_tx));

    // The ksni tray runs on its own thread, so on Linux it can exist before (and between)
    // windows. Elsewhere it needs the running event loop, so it is created with the window.
    let mut tray: Option<TrayIcon> = None;
    if cfg!(target_os = "linux") {
        tray = create_tray(&shell, &mut options);
    }

    let mut show_window = !options.start_minimized || options.close == CloseBehavior::Hide;
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
        rgba: tray::icon_rgba(64),
        width: 64,
        height: 64,
    };
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Gravekeeper")
            .with_app_id(APP_ID)
            .with_inner_size([720.0, 480.0])
            .with_min_inner_size([480.0, 320.0])
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
                Arc::clone(shell),
                core,
                close,
                renderer_name(),
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

fn renderer_name() -> &'static str {
    if cfg!(feature = "glow") {
        "glow (OpenGL)"
    } else {
        "wgpu"
    }
}
