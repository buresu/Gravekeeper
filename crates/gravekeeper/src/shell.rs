//! Window lifecycle shared between the GUI thread and tray callbacks.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use eframe::egui::{self, ViewportCommand};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Quit,
}

/// What closing the main window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseBehavior {
    /// Keep the window and event loop alive, just invisible. Needs the platform to support
    /// hiding windows; Wayland does not.
    Hide,
    /// Destroy the window and leave the event loop; a new window is created on demand. Needs
    /// a tray that runs independently of the event loop (ksni on Linux).
    Destroy,
    /// No tray is available, so closing the window exits the application.
    Quit,
}

impl CloseBehavior {
    pub fn platform_default() -> Self {
        if cfg!(target_os = "linux") {
            Self::Destroy
        } else {
            Self::Hide
        }
    }

    /// `GRAVEKEEPER_CLOSE_BEHAVIOR=hide|destroy|quit` overrides the default (for testing).
    pub fn from_env() -> Self {
        match std::env::var("GRAVEKEEPER_CLOSE_BEHAVIOR").as_deref() {
            Ok("hide") => Self::Hide,
            Ok("destroy") => Self::Destroy,
            Ok("quit") => Self::Quit,
            _ => Self::platform_default(),
        }
    }
}

/// State reachable from any thread (tray callbacks run on their own threads).
pub struct Shell {
    ctx: egui::Context,
    quit: AtomicBool,
    window_open: AtomicBool,
    wake: mpsc::Sender<TrayAction>,
}

impl Shell {
    pub fn new(ctx: egui::Context, wake: mpsc::Sender<TrayAction>) -> Self {
        Self {
            ctx,
            quit: AtomicBool::new(false),
            window_open: AtomicBool::new(false),
            wake,
        }
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.ctx
    }

    pub fn quit_requested(&self) -> bool {
        self.quit.load(Ordering::SeqCst)
    }

    pub fn request_quit(&self) {
        self.quit.store(true, Ordering::SeqCst);
    }

    pub fn set_window_open(&self, open: bool) {
        self.window_open.store(open, Ordering::SeqCst);
    }

    /// Routes a tray action to the running window, or to the main loop if there is none.
    pub fn handle(&self, action: TrayAction) {
        tracing::debug!(?action, "tray action");
        if action == TrayAction::Quit {
            self.request_quit();
        }
        if !self.window_open.load(Ordering::SeqCst) {
            let _ = self.wake.send(action);
            return;
        }
        match action {
            TrayAction::Show => {
                self.ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                self.ctx
                    .send_viewport_cmd(ViewportCommand::Minimized(false));
                self.ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
            TrayAction::Quit => self.ctx.send_viewport_cmd(ViewportCommand::Close),
        }
        self.ctx.request_repaint();
    }
}
