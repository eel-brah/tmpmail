//! tmpmail's desktop app (spike). The window lists the stored inboxes (the
//! CLI's state file), creates new ones, lists their mail and previews one
//! message through the `tmpmail-mail` scheme. See `gui/README.md` for the
//! security model.

// No console window on Windows release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod scheme;
mod worker;

#[cfg(debug_assertions)]
mod probe;

use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Value, json};
use tauri::webview::NewWindowResponse;
use tauri::{Manager, State, Url, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;
use tmpmail::provider::{self, Message};

use crate::worker::Req;

/// How many opened messages the scheme can still serve.
const KEEP_MAILS: usize = 32;

/// State shared by the commands, the scheme handler and the worker.
pub struct Shared {
    pub worker: OnceLock<Sender<Req>>,
    /// Why the worker couldn't start (an unreadable config or state file).
    pub start_error: OnceLock<String>,
    pub mails: Mutex<Mails>,
    #[cfg(debug_assertions)]
    pub probe: Option<probe::Probe>,
}

/// An opened message, kept so the scheme can serve it and its images.
pub struct Mail {
    pub address: String,
    /// Shared so the scheme and the worker can use it without the lock.
    pub msg: Arc<Message>,
    /// Inline images already in memory, by content id: (type, bytes).
    pub inline: HashMap<String, (String, Vec<u8>)>,
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
        Mail {
            address: address.to_string(),
            msg: Arc::new(msg),
            inline: HashMap::new(),
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

fn send(shared: &Shared, req: Req) -> Result<(), String> {
    if let Some(e) = shared.start_error.get() {
        return Err(e.clone());
    }
    shared
        .worker
        .get()
        .ok_or("the worker is not running")?
        .send(req)
        .map_err(|_| "the worker stopped".to_string())
}

/// What the window needs to start: the mail scheme's origin and the
/// providers. Also asks the worker for the inbox list and every inbox's
/// mail, since the window listens for events only once it has loaded.
/// Fails with the reason if the config or state couldn't be loaded.
#[tauri::command]
fn init(shared: State<'_, Arc<Shared>>) -> Result<Value, String> {
    #[cfg(debug_assertions)]
    let probe = shared.probe.as_ref().map(|p| p.on_init(&shared));
    #[cfg(not(debug_assertions))]
    let probe: Option<Value> = None;

    let providers: Vec<Value> = provider::all()
        .filter_map(|id| provider::get(id).ok())
        .map(|p| {
            let caps = p.caps();
            json!({ "id": p.id(), "name": p.name(), "private": caps.private, "attachments": caps.attachments })
        })
        .collect();
    send(&shared, Req::Refresh)?;
    Ok(json!({ "origin": scheme::origin(), "providers": providers, "probe": probe }))
}

#[tauri::command]
fn create_inbox(shared: State<'_, Arc<Shared>>, provider: Option<String>) -> Result<(), String> {
    if let Some(id) = &provider {
        provider::get(id).map_err(|e| e.to_string())?;
    }
    send(&shared, Req::Create { provider })
}

#[tauri::command]
fn select_inbox(shared: State<'_, Arc<Shared>>, address: String) -> Result<(), String> {
    send(&shared, Req::Select { address })
}

#[tauri::command]
fn open_message(shared: State<'_, Arc<Shared>>, address: String, id: String) -> Result<(), String> {
    send(&shared, Req::Open { address, id })
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
    let shared = Arc::new(Shared {
        worker: OnceLock::new(),
        start_error: OnceLock::new(),
        mails: Mutex::new(Mails::default()),
        #[cfg(debug_assertions)]
        probe: probe::Probe::from_env(),
    });

    let for_scheme = shared.clone();
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .register_asynchronous_uri_scheme_protocol(scheme::SCHEME, move |ctx, req, responder| {
            scheme::handle(ctx.app_handle(), &for_scheme, req, responder)
        });
    #[cfg(debug_assertions)]
    let builder = builder.invoke_handler(tauri::generate_handler![
        init,
        create_inbox,
        select_inbox,
        open_message,
        probe::probe_finish
    ]);
    #[cfg(not(debug_assertions))]
    let builder = builder.invoke_handler(tauri::generate_handler![
        init,
        create_inbox,
        select_inbox,
        open_message
    ]);

    let result = builder
        .setup(move |app| {
            app.manage(shared.clone());
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

            let handle = app.handle().clone();
            let for_popups = shared.clone();
            WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("tmpmail")
                .inner_size(1180.0, 760.0)
                .min_inner_size(820.0, 520.0)
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
        .run(tauri::generate_context!());
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
}
