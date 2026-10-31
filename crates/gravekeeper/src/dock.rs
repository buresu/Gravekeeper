//! The Dock icon on macOS, shown only while the window is. With the window hidden Gravekeeper
//! lives in the menu bar (the tray) alone, like other menu bar apps. Elsewhere these are no-ops.
//!
//! The icon follows the app's activation policy: `Regular` shows it, `Accessory` hides it.

/// Starts without a Dock icon when the window starts hidden. Applies only to the first window:
/// winit's event loop, which takes the policy, is created once and then reused.
pub fn configure(options: &mut eframe::NativeOptions, start_hidden: bool) {
    imp::configure(options, start_hidden);
}

/// Shows or hides the Dock icon to match the window. Acts only on the main thread (elsewhere
/// it does nothing, and the window's next frame catches up) and only when the state changes.
pub fn set_window_shown(shown: bool) {
    imp::set_window_shown(shown);
}

#[cfg(target_os = "macos")]
mod imp {
    use std::cell::Cell;

    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};

    thread_local! {
        /// The state last applied; `None` before the first window.
        static SHOWN: Cell<Option<bool>> = const { Cell::new(None) };
    }

    pub fn configure(options: &mut eframe::NativeOptions, start_hidden: bool) {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

        if SHOWN.get().is_some() {
            return;
        }
        SHOWN.set(Some(!start_hidden));
        if start_hidden {
            options.event_loop_builder = Some(Box::new(|builder| {
                builder.with_activation_policy(ActivationPolicy::Accessory);
            }));
        }
    }

    pub fn set_window_shown(shown: bool) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        if SHOWN.replace(Some(shown)) == Some(shown) {
            return;
        }
        let app = NSApplication::sharedApplication(mtm);
        let policy = if shown {
            NSApplicationActivationPolicy::Regular
        } else {
            NSApplicationActivationPolicy::Accessory
        };
        if !app.setActivationPolicy(policy) {
            tracing::warn!(shown, "cannot change the Dock icon");
        }
        if shown {
            // An app without a Dock icon is not active, so its window would open behind the
            // others. `activate` would do, but needs macOS 14.
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    pub fn configure(_options: &mut eframe::NativeOptions, _start_hidden: bool) {}

    pub fn set_window_shown(_shown: bool) {}
}
