//! Window lifecycle shared between the GUI thread and tray callbacks.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use eframe::egui::{self, ViewportCommand};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    Show,
    Hide,
    /// Hide if the window is shown, otherwise show it.
    Toggle,
    Quit,
}

/// What closing the main window does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseBehavior {
    /// Keep the window and event loop alive, just invisible. Needs the platform to support
    /// hiding windows; Wayland does not.
    Hide,
    /// Destroy the window and leave the event loop; a new window is created on demand. Needs
    /// a tray that works without the event loop: ksni on Linux, and on Windows a message loop
    /// that `gui.rs` runs while there is no window.
    Destroy,
    /// No tray is available, so closing the window exits the application.
    Quit,
}

impl CloseBehavior {
    pub fn platform_default() -> Self {
        if cfg!(any(target_os = "linux", windows)) {
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
    window_shown: AtomicBool,
    wake: mpsc::Sender<TrayAction>,
}

impl Shell {
    pub fn new(ctx: egui::Context, wake: mpsc::Sender<TrayAction>) -> Self {
        Self {
            ctx,
            quit: AtomicBool::new(false),
            window_open: AtomicBool::new(false),
            window_shown: AtomicBool::new(false),
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
        if !open {
            self.set_window_shown(false);
        }
    }

    /// Whether the window exists and is not hidden.
    pub fn window_shown(&self) -> bool {
        self.window_shown.load(Ordering::SeqCst)
    }

    pub fn set_window_shown(&self, shown: bool) {
        self.window_shown.store(shown, Ordering::SeqCst);
    }

    /// Routes a tray action to the running window, or to the main loop if there is none.
    pub fn handle(&self, action: TrayAction) {
        let action = match action {
            TrayAction::Toggle if self.window_shown() => TrayAction::Hide,
            TrayAction::Toggle => TrayAction::Show,
            other => other,
        };
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
                self.set_window_shown(true);
            }
            // Like the window's close button: hides or destroys it, depending on `CloseBehavior`.
            TrayAction::Hide => self.ctx.send_viewport_cmd(ViewportCommand::Close),
            TrayAction::Toggle => unreachable!("resolved above"),
            TrayAction::Quit => self.ctx.send_viewport_cmd(ViewportCommand::Close),
        }
        self.ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_shows_a_hidden_or_missing_window() {
        let (tx, rx) = mpsc::channel();
        let shell = Shell::new(egui::Context::default(), tx);
        shell.handle(TrayAction::Toggle);
        assert_eq!(rx.try_recv(), Ok(TrayAction::Show));
    }

    #[test]
    fn toggle_hides_a_shown_window() {
        let (tx, rx) = mpsc::channel();
        let shell = Shell::new(egui::Context::default(), tx);
        shell.set_window_open(true);
        shell.set_window_shown(true);
        shell.handle(TrayAction::Toggle);
        assert!(
            rx.try_recv().is_err(),
            "handled by the window, not the main loop"
        );
        // Hiding is confirmed by the window when it processes the close request.
        assert!(shell.window_shown());
    }

    #[test]
    fn a_closed_window_is_not_shown() {
        let (tx, _rx) = mpsc::channel();
        let shell = Shell::new(egui::Context::default(), tx);
        shell.set_window_open(true);
        shell.set_window_shown(true);
        shell.set_window_open(false);
        assert!(!shell.window_shown());
    }
}
