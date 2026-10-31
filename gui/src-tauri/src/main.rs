//! tmpmail's desktop app. The window shows the stored inboxes (the CLI's
//! state file), their mail as it arrives, and one message at a time through
//! the `tmpmail-mail` scheme. Every privileged action (clipboard, links,
//! saving files) is a command here, never a plugin permission of the page.
//! See `gui/README.md` for the security model.

// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod caller;
mod commands;
mod drag;
#[cfg(target_os = "linux")]
mod freedesktop;
mod notify;
mod scheme;
mod settings;
mod tray;
mod worker;

#[cfg(debug_assertions)]
mod probe;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::webview::NewWindowResponse;
use tauri::{Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_opener::OpenerExt;
use tmpmail::extract;
use tmpmail::provider::Message;
use tmpmail::render::body_text;

use crate::worker::Req;

/// The generated command handler, before the caller check wraps it.
type Handler = Box<dyn Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync>;

/// The program name on Linux, and so the window's X11 class (`tmpmail`,
/// `Tmpmail`) and Wayland app ID: the name of the desktop file the packages
/// install (`<productName>.desktop`, with `StartupWMClass=<productName>`),
/// so the desktop finds the app's icon and name for the window. GTK would
/// take the binary's name, `tmpmail-gui`, which no desktop file has: KWin on
/// Wayland then shows its generic icon. A test checks it is `productName`.
#[cfg(target_os = "linux")]
const PROGRAM: &str = "tmpmail";

/// How many opened messages the scheme can still serve.
const KEEP_MAILS: usize = 32;
/// The window's size on a screen with room for it, and its minimum, in
/// logical pixels.
const WINDOW: (f64, f64) = (1240.0, 780.0);
const WINDOW_MIN: (f64, f64) = (900.0, 560.0);
/// Room left around the window's content in the work area: borders, and
/// the title bar the system adds above the content.
const WINDOW_MARGIN: (f64, f64) = (16.0, 48.0);
/// The margin Tauri's `prevent_overflow` leaves. On Windows it adds the
/// real title bar itself, so less is asked there; either way the window it
/// makes is at least as tall as [`fit_window`]'s minimum.
const OVERFLOW_MARGIN: (f64, f64) = if cfg!(windows) {
    // Tauri's title bar comes from DPI-unaware metrics, a little short at
    // high scaling: some extra room.
    (16.0, 24.0)
} else {
    WINDOW_MARGIN
};

/// The window's size and minimum size for a work area of `work` logical
/// pixels: the defaults, made smaller on a screen too small for them, so
/// the whole window (and a dialog's buttons) stays on screen.
fn fit_window(work: (f64, f64)) -> ((f64, f64), (f64, f64)) {
    let room = (
        (work.0 - WINDOW_MARGIN.0).max(320.0),
        (work.1 - WINDOW_MARGIN.1).max(240.0),
    );
    let min = (WINDOW_MIN.0.min(room.0), WINDOW_MIN.1.min(room.1));
    let size = (WINDOW.0.min(room.0), WINDOW.1.min(room.1));
    (size, min)
}

/// The work area of `monitor` in logical pixels.
fn work_area(monitor: &tauri::window::Monitor) -> (f64, f64) {
    let scale = monitor.scale_factor();
    let area = monitor.work_area().size;
    (
        f64::from(area.width) / scale,
        f64::from(area.height) / scale,
    )
}

/// Lets the window shrink to fit the monitor it is now on (it moved, or the
/// scale changed). Its size is left alone.
fn fit_min_size(window: &tauri::Window) {
    // Moves come by the hundred while the window is dragged: act only when
    // the work area changed.
    static LAST: Mutex<Option<(f64, f64)>> = Mutex::new(None);
    if let Ok(Some(monitor)) = window.current_monitor() {
        let work = work_area(&monitor);
        let mut last = LAST
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if *last == Some(work) {
            return;
        }
        let (_, min) = fit_window(work);
        if window
            .set_min_size(Some(tauri::LogicalSize::new(min.0, min.1)))
            .is_ok()
        {
            *last = Some(work);
        }
    }
}

/// State shared by the commands, the scheme handler and the worker.
pub struct Shared {
    pub worker: OnceLock<Sender<Req>>,
    /// Why the worker couldn't start (an unreadable config or state file).
    pub start_error: OnceLock<String>,
    pub mails: Mutex<Mails>,
    /// The stored inboxes as the worker last saw them (address to provider
    /// id), so commands can refuse addresses that aren't stored.
    pub inboxes: Mutex<HashMap<String, String>>,
    /// The desktop-only settings in force.
    pub gui: Mutex<settings::Gui>,
    /// Whether the window has the focus (no notifications then).
    pub focused: AtomicBool,
    /// The tray icon, while it is on.
    pub tray: Mutex<Option<tray::Tray>>,
    /// Whether a tray icon can be seen (Linux: a StatusNotifier host runs).
    pub tray_host: AtomicBool,
    /// The newest code seen, for the tray's "Copy last code".
    pub last_code: Mutex<Option<tray::LastCode>>,
    /// What a click on each shown notification opens (the plugin's).
    #[cfg(not(target_os = "linux"))]
    pub clicks: Mutex<HashMap<i32, notify::Target>>,
    /// What a click on a notification asked for before the page was up
    /// (Windows: the click started the app); `init` hands it over.
    pub pending: Mutex<notify::Pending>,
    #[cfg(not(target_os = "linux"))]
    pub next_notification: std::sync::atomic::AtomicI32,
    /// Linux: notifications go to the notification server directly.
    #[cfg(target_os = "linux")]
    pub freedesktop: OnceLock<Arc<freedesktop::Notifier>>,
    /// Attachments written out for dragging.
    pub drag: drag::Files,
    #[cfg(debug_assertions)]
    pub probe: Option<probe::Probe>,
}

/// An opened message, kept so the scheme can serve it and its images, and
/// the commands can copy or open what was found in it.
pub struct Mail {
    pub address: String,
    /// Shared so the scheme and the worker can use it without the lock.
    pub msg: Arc<Message>,
    /// The verification code `extract` found, if any.
    pub code: Option<String>,
    /// http(s) links, likely action links first.
    pub links: Vec<String>,
    /// Inline images already in memory, by content id: (type, bytes).
    pub inline: HashMap<String, (String, Vec<u8>)>,
    /// The message source once fetched (raw MIME, or the HTML body).
    pub source: Option<Arc<String>>,
}

/// Opened messages by key. Keys are never reused, so a frame can't show a
/// different message than the one it was opened for.
#[derive(Default)]
pub struct Mails {
    next: u64,
    map: BTreeMap<u64, Mail>,
}

impl Mails {
    pub fn entry(address: &str, msg: Message) -> Mail {
        let body = body_text(msg.text.as_deref(), msg.html.as_deref(), 200);
        let code = extract::otp(&msg.subject, &body, None);
        let links = extract::links(
            msg.text.as_deref().unwrap_or_default(),
            msg.html.as_deref().unwrap_or_default(),
        )
        .into_iter()
        .filter(|l| commands::web_url(l).is_some())
        .collect();
        Mail {
            address: address.to_string(),
            msg: Arc::new(msg),
            code,
            links,
            inline: HashMap::new(),
            source: None,
        }
    }

    pub fn insert(&mut self, mail: Mail) -> u64 {
        self.next += 1;
        self.map.insert(self.next, mail);
        while self.map.len() > KEEP_MAILS {
            self.map.pop_first();
        }
        self.next
    }

    pub fn get(&self, key: u64) -> Option<&Mail> {
        self.map.get(&key)
    }

    pub fn get_mut(&mut self, key: u64) -> Option<&mut Mail> {
        self.map.get_mut(&key)
    }
}

/// Only the app's own pages may load in the main webview. (`Url::origin`
/// is opaque for custom schemes, so scheme and host are compared.)
fn app_page(url: &Url) -> bool {
    if caller::app_page(url) {
        return true;
    }
    // The mail frame's own loads, on engines that report them here
    // (WebView2 maps the scheme to http).
    let host = url.host_str().unwrap_or_default();
    match url.scheme() {
        scheme::SCHEME => host == "localhost",
        "http" => host == "tmpmail-mail.localhost",
        _ => false,
    }
}

/// Mail links open as popups (`target="_blank"`). None is ever created: the
/// link goes to the system browser or mail client, if its scheme is one we
/// trust.
fn popup(app: &tauri::AppHandle, shared: &Shared, url: &Url) {
    #[cfg(debug_assertions)]
    if let Some(p) = &shared.probe {
        p.record_popup(url);
        return;
    }
    let _ = shared;
    if matches!(url.scheme(), "http" | "https" | "mailto")
        && let Err(e) = app.opener().open_url(url.as_str(), None::<&str>)
    {
        eprintln!("tmpmail-gui: could not open {url}: {e}");
    }
}

fn main() {
    // Before GTK starts, which would set it from the binary's name. It is
    // also the tray icon's title on Linux (libappindicator's `Title` falls
    // back to the application name, and that to the program name).
    #[cfg(target_os = "linux")]
    gtk::glib::set_prgname(Some(PROGRAM));
    #[cfg(debug_assertions)]
    let probe = probe::Probe::from_env();
    #[cfg(debug_assertions)]
    let probing = probe.is_some();
    #[cfg(not(debug_assertions))]
    let probing = false;
    // The probe runs with neither a tray nor notifications.
    let gui = if probing {
        settings::Gui {
            notifications: false,
            tray: false,
            start_minimized: false,
        }
    } else {
        settings::load_gui()
    };
    // Started by a click on a notification in Windows' notification center
    // (the app wasn't running): open what it names once the page is up.
    #[cfg(windows)]
    let pending = std::env::args_os()
        .skip(1)
        .find_map(|a| a.to_str().and_then(notify::activation));
    #[cfg(not(windows))]
    let pending: Option<notify::Target> = None;
    let start_hidden = gui.start_minimized && pending.is_none();
    let shared = Arc::new(Shared {
        worker: OnceLock::new(),
        start_error: OnceLock::new(),
        mails: Mutex::new(Mails::default()),
        inboxes: Mutex::new(HashMap::new()),
        gui: Mutex::new(gui.clone()),
        focused: AtomicBool::new(false),
        tray: Mutex::new(None),
        tray_host: AtomicBool::new(true),
        last_code: Mutex::new(None),
        #[cfg(not(target_os = "linux"))]
        clicks: Mutex::new(HashMap::new()),
        pending: Mutex::new(notify::Pending {
            page_ready: false,
            target: pending,
        }),
        #[cfg(not(target_os = "linux"))]
        next_notification: std::sync::atomic::AtomicI32::new(1),
        #[cfg(target_os = "linux")]
        freedesktop: OnceLock::new(),
        drag: drag::Files::default(),
        #[cfg(debug_assertions)]
        probe,
    });

    let for_scheme = shared.clone();
    // Plugins are used from Rust only: the page has no permission for any
    // of them (see capabilities/main.json).
    let builder = tauri::Builder::default();
    // Windows: a second start (a click on a notification launches the app
    // with an activation link) hands its command line to this one and
    // exits. First, so it runs before anything else starts. Release builds
    // only: a debug build (and the probe) runs beside the installed app.
    #[cfg(all(windows, not(debug_assertions)))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
        // Managed on the builder, so there from the start.
        if let Some(shared) = app.try_state::<Arc<Shared>>() {
            notify::relaunched(app, &shared, &args);
        }
    }));
    let builder = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .on_window_event(|window, event| {
            let shared = window.state::<Arc<Shared>>();
            match event {
                WindowEvent::Focused(focused) => shared.focused.store(*focused, Ordering::Relaxed),
                WindowEvent::Moved(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    fit_min_size(window)
                }
                // With the tray on, closing hides the window; mail is still
                // checked, and the tray brings it back.
                WindowEvent::CloseRequested { api, .. } if tray::keeps_running(&shared) => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                _ => {}
            }
        })
        .register_asynchronous_uri_scheme_protocol(scheme::SCHEME, move |ctx, req, responder| {
            scheme::handle(ctx.app_handle(), &for_scheme, req, responder)
        });
    #[cfg(debug_assertions)]
    let commands: Handler = Box::new(tauri::generate_handler![
        commands::init,
        commands::domains,
        commands::create_inbox,
        commands::select_inbox,
        commands::use_inbox,
        commands::delete_inbox,
        commands::open_message,
        commands::delete_message,
        commands::copy_address,
        commands::copy_from_message,
        commands::open_link,
        commands::save_attachment,
        commands::settings,
        commands::save_settings,
        commands::reveal_config,
        commands::prepare_attachment,
        commands::drag_attachment,
        probe::probe_finish
    ]);
    #[cfg(not(debug_assertions))]
    let commands: Handler = Box::new(tauri::generate_handler![
        commands::init,
        commands::domains,
        commands::create_inbox,
        commands::select_inbox,
        commands::use_inbox,
        commands::delete_inbox,
        commands::open_message,
        commands::delete_message,
        commands::copy_address,
        commands::copy_from_message,
        commands::open_link,
        commands::save_attachment,
        commands::settings,
        commands::save_settings,
        commands::reveal_config,
        commands::prepare_attachment,
        commands::drag_attachment
    ]);

    // Every call passes the caller check first: only the app's page in the
    // main window may run a command (see caller.rs).
    #[cfg(debug_assertions)]
    let for_check = shared.clone();
    let builder = builder.invoke_handler(move |invoke| {
        let (_caller, verdict) = caller::check(&invoke.message);
        #[cfg(debug_assertions)]
        if let Some(p) = &for_check.probe {
            p.observe(&invoke.message, &_caller, verdict.is_ok());
        }
        if let Err(e) = verdict {
            invoke.resolver.reject(e);
            return true;
        }
        commands(invoke)
    });

    let for_exit = shared.clone();
    let result = builder
        .manage(shared.clone())
        .setup(move |app| {
            // Before anything can show a notification.
            notify::register(app.handle(), shared.clone());
            #[cfg(debug_assertions)]
            if let Some(p) = &shared.probe {
                p.start(app.handle().clone(), &shared);
            }
            match worker::spawn(app.handle().clone(), shared.clone()) {
                Ok(tx) => {
                    let _ = shared.worker.set(tx);
                }
                Err(e) => {
                    let _ = shared.start_error.set(e);
                }
            }

            let mut hidden = false;
            if gui.tray {
                let host = tray::host_available();
                shared.tray_host.store(host, Ordering::Relaxed);
                match tray::set(app.handle(), &shared, true) {
                    // Not when a notification click is waiting (Windows).
                    Ok(()) => {
                        let waiting = shared
                            .pending
                            .lock()
                            .expect("pending lock")
                            .target
                            .is_some();
                        hidden = start_hidden && host && !waiting;
                    }
                    Err(e) => eprintln!("tmpmail-gui: no tray icon: {e}"),
                }
            }

            // On a screen too small for the default size, Tauri shrinks the
            // window to the work area (the primary monitor's: it opens
            // there) and centers it; the minimum size shrinks with it.
            let fit = app
                .primary_monitor()
                .ok()
                .flatten()
                .map(|m| fit_window(work_area(&m)));
            let (size, min) = fit.unwrap_or((WINDOW, WINDOW_MIN));
            let handle = app.handle().clone();
            let for_popups = shared.clone();
            let mut builder =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("tmpmail")
                    .visible(!hidden)
                    .inner_size(WINDOW.0, WINDOW.1)
                    .min_inner_size(min.0, min.1)
                    .prevent_overflow_with_margin(tauri::LogicalSize::new(
                        OVERFLOW_MARGIN.0,
                        OVERFLOW_MARGIN.1,
                    ));
            if size != WINDOW {
                builder = builder.center();
            }
            let window = builder
                .on_navigation(|url| {
                    let ok = app_page(url);
                    if std::env::var_os("TMPMAIL_GUI_LOG").is_some() {
                        eprintln!(
                            "navigation {url}: {}",
                            if ok { "allowed" } else { "blocked" }
                        );
                    }
                    ok
                })
                .on_page_load(|_, payload| {
                    if std::env::var_os("TMPMAIL_GUI_LOG").is_some() {
                        eprintln!("page {:?}: {}", payload.event(), payload.url());
                    }
                })
                .on_new_window(move |url, _features| {
                    popup(&handle, &for_popups, &url);
                    NewWindowResponse::Deny
                })
                .build()?;
            // Calls must come over the IPC protocol, where the engine sets
            // their Origin (caller.rs). The app doesn't start without that.
            caller::close_post_message(&window)
                .map_err(|e| format!("could not secure the window's IPC: {e}"))?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .map(|app| {
            app.run(move |_, event| {
                if let RunEvent::Exit = event {
                    for_exit.drag.clear();
                }
            })
        });
    if let Err(e) = result {
        eprintln!("tmpmail-gui: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "linux")]
    fn the_program_name_is_the_desktop_files() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["productName"], PROGRAM);
        let linux = &conf["bundle"]["linux"];
        for package in ["deb", "rpm"] {
            assert_eq!(
                linux[package]["desktopTemplate"],
                "packaging/tmpmail.desktop"
            );
        }
        let template = include_str!("../packaging/tmpmail.desktop");
        assert!(template.lines().any(|l| l == "StartupWMClass={{name}}"));
        // The window icon on Linux: Tauri takes the first PNG of the bundle.
        let first_png = conf["bundle"]["icon"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i.as_str().unwrap().ends_with(".png"));
        assert_eq!(first_png.unwrap(), "icons/128x128.png");
    }

    #[test]
    fn only_the_apps_own_pages_load() {
        let ok = |u: &str| app_page(&Url::parse(u).unwrap());
        assert!(ok("tauri://localhost/index.html"));
        assert!(ok("http://tauri.localhost/index.html"));
        assert!(ok("tmpmail-mail://localhost/mail/1"));
        // The mail origin on Windows, in debug builds too.
        assert!(ok("http://tmpmail-mail.localhost/mail/1"));
        assert_eq!(ok("http://localhost:1420/"), cfg!(debug_assertions));
        assert!(!ok("http://localhost:8080/"));
        assert!(!ok("https://example.com/"));
        assert!(!ok("tmpmail-mail://evil/mail/1"));
    }

    #[test]
    fn the_window_fits_small_screens() {
        // Room for the defaults: unchanged.
        assert_eq!(fit_window((1920.0, 1040.0)), (WINDOW, WINDOW_MIN));
        // The test VM: 1280×720 with a 48 px taskbar.
        assert_eq!(fit_window((1280.0, 672.0)), ((1240.0, 624.0), WINDOW_MIN));
        // 1024×600: shorter than the minimum, which shrinks too.
        assert_eq!(
            fit_window((1024.0, 560.0)),
            ((1008.0, 512.0), (900.0, 512.0))
        );
        // A 1366×768 laptop at 150 %: 910×480 logical.
        let (size, min) = fit_window((910.0, 480.0));
        assert_eq!(size, (894.0, 432.0));
        assert_eq!(min, size);
    }

    #[test]
    fn opened_mail_keeps_its_code_and_web_links_only() {
        let msg = Message {
            subject: "Your verification code".into(),
            text: Some(
                "Use 482913 to sign in. https://example.com/verify?t=1 \
                 javascript:alert(1) http://plain.example/x"
                    .into(),
            ),
            ..Default::default()
        };
        let mail = Mails::entry("a@x.com", msg);
        assert_eq!(mail.code.as_deref(), Some("482913"));
        assert_eq!(mail.links[0], "https://example.com/verify?t=1");
        assert!(mail.links.iter().all(|l| l.starts_with("http")));
    }
}
