//! tmpmail's desktop app. The window shows the stored inboxes (the CLI's
//! state file), their mail as it arrives, and one message at a time through
//! the `tmpmail-mail` scheme. Every privileged action (clipboard, links,
//! saving files) is a command here, never a plugin permission of the page.
//! See `gui/README.md` for the security model.

// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod drag;
mod notify;
mod scheme;
mod settings;
mod tray;
mod worker;

#[cfg(debug_assertions)]
mod probe;

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use tauri::webview::NewWindowResponse;
use tauri::{Manager, RunEvent, Url, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_opener::OpenerExt;
use tmpmail::extract;
use tmpmail::provider::Message;
use tmpmail::render::body_text;

use crate::worker::Req;

/// How many opened messages the scheme can still serve.
const KEEP_MAILS: usize = 32;

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
    /// What a click on each shown notification opens.
    pub clicks: Mutex<HashMap<i32, notify::Target>>,
    pub next_notification: AtomicI32,
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
    let host = url.host_str().unwrap_or_default();
    match url.scheme() {
        "tauri" => host == "localhost",
        "http" | "https" if host == "tauri.localhost" => true,
        // The mail frame's own loads, on engines that report them here
        // (WebView2 maps the scheme to http), and Vite in debug builds.
        scheme::SCHEME => host == "localhost",
        "http" => {
            host == "tmpmail-mail.localhost"
                || cfg!(debug_assertions) && host == "localhost" && url.port() == Some(1420)
        }
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
        clicks: Mutex::new(HashMap::new()),
        next_notification: AtomicI32::new(1),
        drag: drag::Files::default(),
        #[cfg(debug_assertions)]
        probe,
    });

    let for_scheme = shared.clone();
    // Plugins are used from Rust only: the page has no permission for any
    // of them (see capabilities/main.json).
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .on_window_event(|window, event| {
            let shared = window.state::<Arc<Shared>>();
            match event {
                WindowEvent::Focused(focused) => shared.focused.store(*focused, Ordering::Relaxed),
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
    let builder = builder.invoke_handler(tauri::generate_handler![
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
    let builder = builder.invoke_handler(tauri::generate_handler![
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

    let for_exit = shared.clone();
    let result = builder
        .setup(move |app| {
            app.manage(shared.clone());
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
                    Ok(()) => hidden = gui.start_minimized && host,
                    Err(e) => eprintln!("tmpmail-gui: no tray icon: {e}"),
                }
            }

            let handle = app.handle().clone();
            let for_popups = shared.clone();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("tmpmail")
                .visible(!hidden)
                .inner_size(1240.0, 780.0)
                .min_inner_size(900.0, 560.0)
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
