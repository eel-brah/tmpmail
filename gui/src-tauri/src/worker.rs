//! The background thread that does all network work, modelled on the TUI's
//! worker: its own `App`, requests over a channel, every stored inbox polled
//! (with push where the provider has it), and the state file reloaded every
//! few seconds so changes made by the CLI show up. Results go to the window
//! as events.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use serde_json::json;
use tauri::{AppHandle, Emitter, UriSchemeResponder};
use tmpmail::app::{App, Global};
use tmpmail::provider::CreateOpts;
use tmpmail::push::Poller;

use crate::scheme;
use crate::{Mails, Shared};

/// How often the state file is re-read for changes made elsewhere.
const STATE_RELOAD: Duration = Duration::from_secs(5);

pub enum Req {
    /// Send the inbox list again (the window just loaded).
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

pub fn spawn(handle: AppHandle, shared: std::sync::Arc<Shared>) -> Sender<Req> {
    let (tx, rx) = channel();
    std::thread::Builder::new()
        .name("tmpmail-worker".into())
        .spawn(move || {
            if let Err(e) = run(&handle, &shared, rx) {
                notice(&handle, "error", &format!("could not start: {e}"));
            }
        })
        .expect("spawn the worker thread");
    tx
}

fn notice(handle: &AppHandle, kind: &str, text: &str) {
    let _ = handle.emit("notice", json!({ "kind": kind, "text": text }));
}

fn send_inboxes(handle: &AppHandle, app: &App) {
    let inboxes: Vec<_> = app.state.inboxes.iter().map(|i| app.view(i)).collect();
    let _ = handle.emit("inboxes", json!({ "inboxes": inboxes }));
}

fn run(handle: &AppHandle, shared: &Shared, rx: Receiver<Req>) -> tmpmail::error::Result<()> {
    let mut app = App::new(Global::default())?;
    let (log_tx, log_rx) = channel();
    app.log_to(log_tx);
    let interval = Duration::from_secs(app.config.poll_interval(None));
    let mut poller = Poller::new(interval, app.config.push());
    let mut reloaded = Instant::now();
    loop {
        // Of several opens only the last matters: the user moved on.
        let mut reqs: Vec<Req> = rx.try_iter().collect();
        if let Some(last) = reqs.iter().rposition(|r| matches!(r, Req::Open { .. })) {
            let mut k = 0;
            reqs.retain(|r| {
                k += 1;
                !matches!(r, Req::Open { .. }) || k - 1 == last
            });
        }
        for req in reqs {
            if handle_req(handle, shared, &mut app, &mut poller, req) {
                send_inboxes(handle, &app);
            }
        }
        for note in log_rx.try_iter() {
            notice(handle, "info", &note);
        }

        if reloaded.elapsed() >= STATE_RELOAD {
            reloaded = Instant::now();
            if let Ok(state) = app.store.load() {
                let addresses = |s: &tmpmail::store::State| -> Vec<(String, Option<String>)> {
                    s.inboxes
                        .iter()
                        .map(|i| (i.address.clone(), i.label.clone()))
                        .collect()
                };
                let changed =
                    addresses(&state) != addresses(&app.state) || state.active != app.state.active;
                app.state = state;
                if changed {
                    send_inboxes(handle, &app);
                }
            }
        }

        for n in 0..app.state.inboxes.len() {
            let address = app.state.inboxes[n].address.clone();
            if app.state.inboxes[n].is_expired() || !poller.is_due(&address) {
                continue;
            }
            let result = app.with_inbox_at(n, |p, inbox| {
                let list = App::sorted_list(p, inbox)?;
                poller.subscribe(p, inbox);
                Ok(list)
            });
            poller.polled(&address, result.is_ok());
            let payload = match result {
                Ok(list) => {
                    json!({ "address": address, "live": poller.is_live(&address), "messages": list })
                }
                Err(e) => json!({ "address": address, "error": e.to_string() }),
            };
            if handle.emit("messages", payload).is_err() {
                return Ok(()); // the window is gone
            }
        }
        // Short naps so requests from the window are picked up quickly.
        poller.sleep(Some(Instant::now() + Duration::from_millis(100)));
    }
}

/// Carries out one request. Returns whether the inbox list changed.
fn handle_req(
    handle: &AppHandle,
    shared: &Shared,
    app: &mut App,
    poller: &mut Poller,
    req: Req,
) -> bool {
    match req {
        Req::Refresh => true,
        Req::Select { address } => {
            poller.forget(&address); // due now
            false
        }
        Req::Create { provider } => {
            match app.create_with_fallback(provider.as_deref(), None, &CreateOpts::default()) {
                Ok(inbox) => {
                    let address = inbox.address.clone();
                    match app.store.update(|s| s.add(inbox)) {
                        Ok((state, ())) => {
                            app.state = state;
                            let _ = handle.emit("created", json!({ "address": address }));
                        }
                        Err(e) => {
                            notice(handle, "error", &format!("could not save the inbox: {e}"))
                        }
                    }
                }
                Err(e) => notice(handle, "error", &format!("could not create an inbox: {e}")),
            }
            true
        }
        Req::Open { address, id } => {
            let Some(n) = app.state.position(&address) else {
                return false;
            };
            let payload = match app.with_inbox_at(n, |p, inbox| p.get(inbox, &id)) {
                Ok(msg) => {
                    let key = shared
                        .mails
                        .lock()
                        .expect("mails lock")
                        .insert(Mails::entry(&address, msg.clone()));
                    json!({
                        "address": address,
                        "id": id,
                        "key": key,
                        "from": msg.from,
                        "to": msg.to,
                        "subject": msg.subject,
                        "date": msg.date,
                        "attachments": msg.attachments,
                    })
                }
                Err(e) => json!({ "address": address, "id": id, "error": e.to_string() }),
            };
            let _ = handle.emit("opened", payload);
            false
        }
        Req::Inline {
            key,
            cid,
            responder,
        } => {
            let found = {
                let mails = shared.mails.lock().expect("mails lock");
                mails.get(key).and_then(|m| {
                    let att = scheme::find_inline(&m.msg, &cid)?.clone();
                    Some((m.address.clone(), m.msg.clone(), att))
                })
            };
            let Some((address, msg, att)) = found else {
                responder.respond(scheme::not_found());
                return false;
            };
            let bytes = app.state.position(&address).and_then(|n| {
                app.with_inbox_at(n, |p, inbox| p.attachment(inbox, &msg, &att))
                    .ok()
            });
            responder.respond(match bytes {
                Some(b) => scheme::image(&att.content_type, b),
                None => scheme::not_found(),
            });
            false
        }
    }
}
