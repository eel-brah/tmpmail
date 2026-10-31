//! The background thread that does all network work: the core's shared
//! worker loop (`tmpmail::watcher`, also behind the TUI) with the window's
//! requests and events. It owns its own `App`, polls every stored inbox
//! (with push where the provider has it), and re-reads the state file every
//! few seconds so changes made by the CLI show up. Results go to the window
//! as events.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};

use serde_json::{Value, json};
use tauri::{AppHandle, Emitter, UriSchemeResponder};
use tmpmail::app::{App, Global};
use tmpmail::provider::Summary;
use tmpmail::watcher::{self, Watcher};

use crate::scheme;
use crate::{Mails, Shared};

pub enum Req {
    /// The window just loaded: send the inbox list and every inbox's last
    /// listing again, since events sent before it listened are lost.
    Refresh,
    Create {
        provider: Option<String>,
    },
    /// List this inbox now instead of on its next tick.
    Select {
        address: String,
    },
    Open {
        address: String,
        id: String,
    },
    /// An inline image of an opened message, for the mail scheme.
    Inline {
        key: u64,
        cid: String,
        responder: UriSchemeResponder,
    },
}

/// Starts the worker once its `App` has loaded the config and state, or
/// returns why they couldn't be loaded.
pub fn spawn(handle: AppHandle, shared: Arc<Shared>) -> Result<Sender<Req>, String> {
    let (tx, rx) = channel();
    let (ready_tx, ready_rx) = channel();
    std::thread::Builder::new()
        .name("tmpmail-worker".into())
        .spawn(move || {
            // Built here: an `App` can't move between threads.
            let mut w = match Watcher::new(Global::default()) {
                Ok(w) => w,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.to_string()));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            let (log_tx, log) = channel();
            w.app.log_to(log_tx);
            w.run(
                &rx,
                &mut Front {
                    handle,
                    shared,
                    log,
                    last: HashMap::new(),
                },
            );
        })
        .map_err(|e| format!("could not start the worker: {e}"))?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(tx),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("the worker stopped".into()),
    }
}

/// The window's side of the shared loop.
struct Front {
    handle: AppHandle,
    shared: Arc<Shared>,
    log: Receiver<String>,
    /// The last `messages` event of each inbox, for `Req::Refresh`.
    last: HashMap<String, Value>,
}

impl Front {
    fn messages(&mut self, address: String, payload: Value) -> bool {
        let ok = self.handle.emit("messages", &payload).is_ok();
        self.last.insert(address, payload);
        ok
    }

    fn notice(&self, kind: &str, text: &str) {
        let _ = self
            .handle
            .emit("notice", json!({ "kind": kind, "text": text }));
    }
}

impl watcher::Front for Front {
    type Req = Req;

    fn is_open(req: &Req) -> bool {
        matches!(req, Req::Open { .. })
    }

    fn inboxes(&mut self, app: &App) -> bool {
        let inboxes: Vec<_> = app.state.inboxes.iter().map(|i| app.view(i)).collect();
        self.handle
            .emit("inboxes", json!({ "inboxes": inboxes }))
            .is_ok()
    }

    fn listed(
        &mut self,
        address: String,
        result: Result<Vec<Summary>, String>,
        live: bool,
    ) -> bool {
        let payload = match result {
            Ok(list) => json!({ "address": address, "live": live, "messages": list }),
            Err(e) => json!({ "address": address, "live": live, "error": e }),
        };
        self.messages(address, payload)
    }

    fn expired(&mut self, address: &str) -> bool {
        let payload = json!({ "address": address, "expired": true });
        self.messages(address.to_string(), payload)
    }

    fn turn(&mut self) {
        let notes: Vec<String> = self.log.try_iter().collect();
        for note in notes {
            self.notice("info", &note);
        }
    }

    fn handle(&mut self, w: &mut Watcher, req: Req) -> bool {
        match req {
            Req::Refresh => {
                // Inboxes not listed yet are listed on their own soon.
                for inbox in &w.app.state.inboxes {
                    if let Some(payload) = self.last.get(&inbox.address) {
                        let _ = self.handle.emit("messages", payload);
                    }
                }
                true
            }
            Req::Select { address } => {
                w.poll_now(&address);
                false
            }
            Req::Create { provider } => {
                match w.create(provider.as_deref()) {
                    Ok(address) => {
                        let _ = self.handle.emit("created", json!({ "address": address }));
                    }
                    Err(e) => self.notice("error", &e),
                }
                true
            }
            Req::Open { address, id } => {
                self.open(&mut w.app, address, id);
                false
            }
            Req::Inline {
                key,
                cid,
                responder,
            } => {
                self.inline(&mut w.app, key, cid, responder);
                false
            }
        }
    }
}

impl Front {
    fn open(&self, app: &mut App, address: String, id: String) {
        let Some(n) = app.state.position(&address) else {
            return;
        };
        let payload = match app.with_inbox_at(n, |p, inbox| p.get(inbox, &id)) {
            Ok(msg) => {
                let mut event = json!({
                    "address": address,
                    "id": id,
                    "from": msg.from,
                    "to": msg.to,
                    "subject": msg.subject,
                    "date": msg.date,
                    "attachments": msg.attachments,
                });
                let key = self
                    .shared
                    .mails
                    .lock()
                    .expect("mails lock")
                    .insert(Mails::entry(&address, msg));
                event["key"] = json!(key);
                event
            }
            Err(e) => json!({ "address": address, "id": id, "error": e.to_string() }),
        };
        let _ = self.handle.emit("opened", payload);
    }

    /// Serves an inline image, fetching it from the provider the first
    /// time and keeping it with the message after that.
    fn inline(&self, app: &mut App, key: u64, cid: String, responder: UriSchemeResponder) {
        let found = {
            let mails = self.shared.mails.lock().expect("mails lock");
            mails.get(key).map(|m| {
                (
                    m.inline.get(&cid).cloned(),
                    m.address.clone(),
                    m.msg.clone(),
                )
            })
        };
        let (address, msg) = match found {
            None => return responder.respond(scheme::not_found()),
            // Fetched while this request was queued.
            Some((Some((mime, bytes)), ..)) => {
                return responder.respond(scheme::image(&mime, bytes));
            }
            Some((None, address, msg)) => (address, msg),
        };
        let Some((att, mime)) = scheme::find_inline(&msg, &cid) else {
            return responder.respond(scheme::not_found());
        };
        let bytes = app.state.position(&address).and_then(|n| {
            app.with_inbox_at(n, |p, inbox| p.attachment(inbox, &msg, att))
                .ok()
        });
        let Some(bytes) = bytes else {
            return responder.respond(scheme::not_found());
        };
        if let Some(m) = self.shared.mails.lock().expect("mails lock").get_mut(key) {
            m.inline.insert(cid, (mime.clone(), bytes.clone()));
        }
        responder.respond(scheme::image(&mime, bytes));
    }
}
