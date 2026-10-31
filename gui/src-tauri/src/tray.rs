//! The optional tray icon (off by default, turned on in the settings). Its
//! menu does everything, since Linux reports no clicks on the icon itself:
//! show the window, a new address (copied, with a notification), copy the
//! newest code of the last 15 minutes, quit. While it is on, closing the
//! window hides it and the worker keeps checking mail.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Wry};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tmpmail::provider::CreateOpts;

use crate::worker::Req;
use crate::{Shared, notify};

const ID: &str = "tmpmail";
/// How long "Copy last code" offers a code.
pub const CODE_FRESH: Duration = Duration::from_secs(15 * 60);

/// The tray while it is shown: the menu item that changes.
pub struct Tray {
    code: MenuItem<Wry>,
    /// What `code` shows now, so it is only updated when that changes.
    offered: Option<String>,
}

/// The newest code seen, and when its mail was sent.
#[derive(Debug, Clone)]
pub struct LastCode {
    pub code: String,
    pub at: chrono::DateTime<chrono::Utc>,
}

impl LastCode {
    pub fn fresh(&self) -> bool {
        let age = chrono::Utc::now().signed_duration_since(self.at);
        age.to_std().is_ok_and(|a| a < CODE_FRESH)
    }
}

/// Records a code if its mail is newer than the last one's.
pub fn saw_code(shared: &Shared, code: &str, at: Option<chrono::DateTime<chrono::Utc>>) {
    let at = recorded_at(at, chrono::Utc::now());
    let mut last = shared.last_code.lock().expect("last code lock");
    if last.as_ref().is_none_or(|l| at >= l.at) {
        *last = Some(LastCode {
            code: code.to_string(),
            at,
        });
    }
}

/// When a code counts as seen: its mail's date, but never later than now,
/// so a date in the future (a skewed or lying sender) can neither stay
/// fresh forever nor hide newer codes.
fn recorded_at(
    at: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
) -> chrono::DateTime<chrono::Utc> {
    at.map_or(now, |at| at.min(now))
}

/// The code "Copy last code" offers now, if any.
fn offer(shared: &Shared) -> Option<String> {
    shared
        .last_code
        .lock()
        .expect("last code lock")
        .as_ref()
        .filter(|c| c.fresh())
        .map(|c| c.code.clone())
}

/// Updates the menu's code item when the offered code changed or got old.
/// Called from the worker every turn; cheap when nothing changed.
pub fn refresh(shared: &Shared) {
    let now = offer(shared);
    let item = {
        let mut tray = shared.tray.lock().expect("tray lock");
        let Some(t) = tray.as_mut() else { return };
        if t.offered == now {
            return;
        }
        t.offered = now.clone();
        t.code.clone()
    };
    // Outside the lock: menu calls wait for the main thread.
    let _ = item.set_enabled(now.is_some());
    let _ = item.set_text(match &now {
        Some(code) => format!("Copy code {code}"),
        None => "Copy last code".into(),
    });
}

/// Whether the desktop can show a tray icon. On Linux that takes a
/// StatusNotifier host (KDE, most others; GNOME only with the AppIndicator
/// extension); without one the icon would be invisible, so closing the
/// window must still quit.
#[cfg(target_os = "linux")]
pub fn host_available() -> bool {
    let Ok(bus) = zbus::blocking::Connection::session() else {
        return false;
    };
    bus.call_method(
        Some("org.freedesktop.DBus"),
        "/org/freedesktop/DBus",
        Some("org.freedesktop.DBus"),
        "NameHasOwner",
        &("org.kde.StatusNotifierWatcher",),
    )
    .ok()
    .and_then(|reply| reply.body().deserialize::<bool>().ok())
    .unwrap_or(false)
}

#[cfg(not(target_os = "linux"))]
pub fn host_available() -> bool {
    true
}

/// Whether closing the window should hide it instead of quitting. The tray
/// host is checked again (it may have started after tmpmail, at login).
pub fn keeps_running(shared: &Shared) -> bool {
    if shared.tray.lock().expect("tray lock").is_none() {
        return false;
    }
    let host = host_available();
    shared.tray_host.store(host, Ordering::Relaxed);
    host
}

/// Shows or removes the tray icon. Call on the main thread.
pub fn set(app: &AppHandle, shared: &Arc<Shared>, on: bool) -> tauri::Result<()> {
    let mut tray = shared.tray.lock().expect("tray lock");
    match (on, tray.is_some()) {
        (true, false) => *tray = Some(build(app, shared)?),
        (false, true) => {
            *tray = None;
            let _ = app.remove_tray_by_id(ID);
        }
        _ => return Ok(()),
    }
    drop(tray);
    refresh(shared);
    Ok(())
}

fn build(app: &AppHandle, shared: &Arc<Shared>) -> tauri::Result<Tray> {
    let show = MenuItem::with_id(app, "show", "Show tmpmail", true, None::<&str>)?;
    let new = MenuItem::with_id(app, "new", "New address", true, None::<&str>)?;
    let code = MenuItem::with_id(app, "code", "Copy last code", false, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit tmpmail", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &show,
            &PredefinedMenuItem::separator(app)?,
            &new,
            &code,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    // macOS draws menu bar icons from a black-and-clear template image.
    let icon = if cfg!(target_os = "macos") {
        Image::from_bytes(include_bytes!("../icons/tray-template.png"))?
    } else {
        Image::from_bytes(include_bytes!("../icons/tray.png"))?
    };
    let for_menu = shared.clone();
    TrayIconBuilder::with_id(ID)
        .icon(icon)
        .icon_as_template(cfg!(target_os = "macos"))
        .tooltip("tmpmail")
        .menu(&menu)
        // Windows: a click shows the window, the menu is on right click.
        .show_menu_on_left_click(!cfg!(windows))
        .on_menu_event(move |app, event| menu_event(app, &for_menu, event.id.as_ref()))
        .on_tray_icon_event(|icon, event| {
            if cfg!(windows)
                && let TrayIconEvent::Click {
                    button: tauri::tray::MouseButton::Left,
                    button_state: tauri::tray::MouseButtonState::Up,
                    ..
                } = event
            {
                notify::reveal(icon.app_handle(), None);
            }
        })
        .build(app)?;
    Ok(Tray {
        code,
        offered: None,
    })
}

fn menu_event(app: &AppHandle, shared: &Arc<Shared>, id: &str) {
    match id {
        "show" => notify::reveal(app, None),
        "new" => {
            let req = Req::Create {
                provider: None,
                opts: CreateOpts::default(),
                label: None,
                from_tray: true,
            };
            if let Err(e) = crate::commands::send(shared, req) {
                notify::show(app, shared, "Couldn't create an address", &e, None);
            }
        }
        "code" => {
            let Some(code) = offer(shared) else { return };
            let app = app.clone();
            // Never on the main thread: the clipboard can deadlock there on Linux.
            std::thread::spawn(move || {
                if let Err(e) = app.clipboard().write_text(code) {
                    eprintln!("tmpmail-gui: could not copy the code: {e}");
                }
            });
        }
        "quit" => app.exit(0),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_offered_for_fifteen_minutes() {
        let at = |mins: i64| chrono::Utc::now() - chrono::Duration::minutes(mins);
        let code = |mins| LastCode {
            code: "1".into(),
            at: at(mins),
        };
        assert!(code(0).fresh());
        assert!(code(14).fresh());
        assert!(!code(16).fresh());
        // Dates in the future are taken as now when recorded.
        let now = chrono::Utc::now();
        assert_eq!(recorded_at(Some(at(-60 * 24 * 400)), now), now);
        assert_eq!(recorded_at(None, now), now);
        let past = at(3);
        assert_eq!(recorded_at(Some(past), now), past);
    }
}
