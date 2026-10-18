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

/// Creates the tray icon and routes its events to `shell`.
///
/// On Windows and macOS this must run on the main thread while the event loop is running.
/// On Linux (ksni backend) it can run at any time; the tray lives on its own thread.
pub fn create(shell: &Arc<Shell>) -> Result<TrayIcon, Box<dyn std::error::Error>> {
    if !host_available() {
        return Err("no system tray host is running".into());
    }
    let show = MenuItem::with_id("show", "Show Gravekeeper", true, None);
    let quit = MenuItem::with_id("quit", "Quit", true, None);
    let menu = Menu::new();
    menu.append_items(&[&show, &PredefinedMenuItem::separator(), &quit])?;

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
        "show" => on_menu.handle(TrayAction::Show),
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

    Ok(tray)
}
