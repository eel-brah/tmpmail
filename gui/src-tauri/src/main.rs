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
    pub mails: Mutex<Mails>,
    #[cfg(debug_assertions)]
    pub probe: Option<probe::Probe>,
}

/// An opened message, kept so the scheme can serve it and its images.
pub struct Mail {
    pub address: String,
    pub msg: Message,
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
            msg,
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
}

fn send(shared: &Shared, req: Req) -> Result<(), String> {
    shared
        .worker
        .get()
        .ok_or("the worker is not running")?
        .send(req)
        .map_err(|_| "the worker stopped".to_string())
}

/// What the window needs to start: the mail scheme's origin and the
/// providers. Also asks the worker for the inbox list, since the window
/// listens for events only once it has loaded.
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
        "http" if cfg!(debug_assertions) => host == "localhost" && url.port() == Some(1420),
        // The mail frame's own loads, on engines that report them here.
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
    let shared = Arc::new(Shared {
        worker: OnceLock::new(),
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
            let tx = worker::spawn(app.handle().clone(), shared.clone());
            let _ = shared.worker.set(tx);

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
