use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::shell::{Shell, TrayAction};

const ICON_SIZE: u32 = 32;

/// Whether a tray can be shown. On Linux this needs a StatusNotifierItem host (KDE, Xfce, ...;
/// GNOME only with an extension). Without one the icon would silently not appear.
pub fn host_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        let has_watcher = || -> zbus::Result<bool> {
            let conn = zbus::blocking::Connection::session()?;
            let dbus = zbus::blocking::fdo::DBusProxy::new(&conn)?;
            Ok(dbus.name_has_owner("org.kde.StatusNotifierWatcher".try_into()?)?)
        };
        match has_watcher() {
            Ok(found) => found,
            Err(err) => {
                tracing::warn!(%err, "cannot query the session bus for a tray host");
                false
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    true
}

/// The tray icon; it disappears when this is dropped.
pub struct Tray {
    _icon: TrayIcon,
    window_item: WindowItem,
}

impl Tray {
    pub fn window_item(&self) -> WindowItem {
        self.window_item.clone()
    }
}

/// The menu entry that shows or hides the window. Only usable on the thread that created the
/// tray.
#[derive(Clone)]
pub struct WindowItem {
    item: MenuItem,
    shown: Rc<Cell<bool>>,
}

impl WindowItem {
    /// Labels the entry for the window's current state.
    pub fn set_shown(&self, shown: bool) {
        if self.shown.replace(shown) != shown {
            self.item.set_text(window_item_label(shown));
        }
    }
}

fn window_item_label(shown: bool) -> &'static str {
    if shown {
        "Hide Gravekeeper"
    } else {
        "Show Gravekeeper"
    }
}

/// Creates the tray icon and routes its events to `shell`.
///
/// On Windows and macOS this must run on the main thread while the event loop is running.
/// On Linux (ksni backend) it can run at any time; the tray lives on its own thread.
pub fn create(shell: &Arc<Shell>) -> Result<Tray, Box<dyn std::error::Error>> {
    if !host_available() {
        return Err("no system tray host is running".into());
    }
    let shown = shell.window_shown();
    let window = MenuItem::with_id("window", window_item_label(shown), true, None);
    let quit = MenuItem::with_id("quit", "Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[&window, &PredefinedMenuItem::separator(), &quit])?;

    let icon = tray_icon::Icon::from_rgba(crate::icon::rgba(ICON_SIZE), ICON_SIZE, ICON_SIZE)
        .expect("generated icon has a valid size");

    let tray = TrayIconBuilder::new()
        .with_id("gravekeeper")
        .with_title("Gravekeeper")
        .with_tooltip("Gravekeeper")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;

    let on_menu = Arc::clone(shell);
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| match event.id.as_ref() {
        "window" => on_menu.handle(TrayAction::Toggle),
        "quit" => on_menu.handle(TrayAction::Quit),
        _ => {}
    }));
    let on_click = Arc::clone(shell);
    TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            on_click.handle(TrayAction::Show);
        }
    }));

    Ok(Tray {
        _icon: tray,
        window_item: WindowItem {
            item: window,
            shown: Rc::new(Cell::new(shown)),
        },
    })
}
