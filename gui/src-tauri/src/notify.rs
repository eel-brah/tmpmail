//! Notifications for new mail, shown while the window isn't focused (or is
//! hidden in the tray). The worker spots new mail in each listing; mail that
//! arrives within a couple of seconds is grouped into one notification.
//! Clicking it brings the window back and opens the message. Sender and
//! subject are the sender's words: they are cleaned (`clean_line`) and capped
//! before they reach the notification server.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tmpmail::extract;
use tmpmail::provider::Summary;
use tmpmail::render::clean_line;

use crate::Shared;

/// Mail arriving this soon after the first of a burst joins its notification.
const GROUP: Duration = Duration::from_secs(2);
/// Longest sender, subject and code shown, in characters.
const SENDER_MAX: usize = 48;
const SUBJECT_MAX: usize = 100;
const CODE_MAX: usize = 32;
/// How many notifications remember what a click opens.
const KEEP_CLICKS: usize = 64;

/// A message that just arrived.
#[derive(Debug, Clone)]
pub struct Arrival {
    pub address: String,
    pub id: String,
    /// Display name (or address) of the sender, cleaned.
    pub sender: String,
    pub subject: String,
    /// The code `extract` found in the subject and preview, if any.
    pub code: Option<String>,
    pub date: Option<chrono::DateTime<chrono::Utc>>,
}

/// What a click on a notification opens.
#[derive(Debug, Clone)]
pub struct Target {
    pub address: String,
    pub id: Option<String>,
}

/// New-mail detection and grouping, owned by the worker.
#[derive(Default)]
pub struct Arrivals {
    /// Message ids of each inbox's last listing.
    known: HashMap<String, HashSet<String>>,
    pending: Vec<Arrival>,
    since: Option<Instant>,
}

impl Arrivals {
    /// The messages in `list` that weren't in the inbox's last listing.
    /// An inbox's first listing only records what is already there.
    pub fn listed(&mut self, address: &str, list: &[Summary]) -> Vec<Arrival> {
        let ids: HashSet<String> = list.iter().map(|s| s.id.clone()).collect();
        let arrived = match self.known.get(address) {
            None => Vec::new(),
            Some(known) => list
                .iter()
                .filter(|s| !known.contains(&s.id))
                .map(|s| arrival(address, s))
                .collect(),
        };
        self.known.insert(address.to_string(), ids);
        arrived
    }

    /// Stops tracking inboxes that are gone.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.known.retain(|a, _| keep(a));
        self.pending.retain(|a| keep(&a.address));
    }

    /// Queues mail for the next notification.
    pub fn queue(&mut self, arrived: Vec<Arrival>) {
        if arrived.is_empty() {
            return;
        }
        self.since.get_or_insert_with(Instant::now);
        // Newest first, across inboxes too.
        self.pending.splice(0..0, arrived);
    }

    /// The queued mail once its burst is over.
    pub fn due(&mut self) -> Option<Vec<Arrival>> {
        if self.since?.elapsed() < GROUP {
            return None;
        }
        self.since = None;
        Some(std::mem::take(&mut self.pending))
    }
}

fn arrival(address: &str, s: &Summary) -> Arrival {
    Arrival {
        address: address.to_string(),
        id: s.id.clone(),
        sender: cap(&sender_name(&s.from), SENDER_MAX),
        subject: cap(&clean_line(&s.subject), SUBJECT_MAX),
        code: extract::otp(&s.subject, &s.intro, None).map(|c| cap(&clean_line(&c), CODE_MAX)),
        date: s.date,
    }
}

/// "GitHub" for `GitHub <noreply@github.com>`, else the address, cleaned.
pub fn sender_name(from: &str) -> String {
    let from = clean_line(from);
    let name = from
        .rsplit_once('<')
        .map(|(name, _)| name.trim().trim_matches('"').trim())
        .filter(|n| !n.is_empty());
    match name {
        Some(n) => n.to_string(),
        None => from
            .trim_matches(|c| c == '<' || c == '>')
            .trim()
            .to_string(),
    }
}

/// At most `max` characters, with an ellipsis when cut.
pub fn cap(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// The title and body of the notification for `batch` (newest first).
pub fn text(batch: &[Arrival]) -> (String, String) {
    let subject = |a: &Arrival| {
        if a.subject.is_empty() {
            "(no subject)".to_string()
        } else {
            a.subject.clone()
        }
    };
    let coded = batch.iter().find(|a| a.code.is_some());
    match batch {
        [one] => {
            let title = match &one.code {
                Some(code) => format!("Code {code} from {}", one.sender),
                None => one.sender.clone(),
            };
            (title, subject(one))
        }
        _ => {
            let n = batch.len();
            let title = match coded {
                Some(a) => format!(
                    "Code {} from {}, and {} more",
                    a.code.as_deref().unwrap_or_default(),
                    a.sender,
                    n - 1
                ),
                None => format!("{n} new messages"),
            };
            let mut lines: Vec<String> = batch
                .iter()
                .take(3)
                .map(|a| format!("{}: {}", a.sender, subject(a)))
                .collect();
            if n > 3 {
                lines.push(format!("and {} more", n - 3));
            }
            (title, lines.join("\n"))
        }
    }
}

/// What a click on the notification for `batch` opens: the newest message
/// with a code, else the newest message.
pub fn target(batch: &[Arrival]) -> Option<Target> {
    let a = batch.iter().find(|a| a.code.is_some()).or(batch.first())?;
    Some(Target {
        address: a.address.clone(),
        id: Some(a.id.clone()),
    })
}

/// Escapes the body for servers that read it as markup (most Linux ones),
/// so a subject like `<img src=…>` is shown, never loaded.
fn body_for_server(body: &str) -> String {
    #[cfg(target_os = "linux")]
    {
        use std::sync::OnceLock;
        static MARKUP: OnceLock<bool> = OnceLock::new();
        let markup = *MARKUP.get_or_init(|| {
            notify_rust::get_capabilities()
                .map(|caps| caps.iter().any(|c| c == "body-markup"))
                // Unknown: escape, since the common servers parse markup.
                .unwrap_or(true)
        });
        if markup {
            return escape_markup(body);
        }
    }
    body.to_string()
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn escape_markup(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Shows a notification that opens `target` when clicked.
pub fn show(app: &AppHandle, shared: &Shared, title: &str, body: &str, target: Option<Target>) {
    let id = shared.next_notification.fetch_add(1, Ordering::Relaxed);
    if let Some(t) = target {
        let mut clicks = shared.clicks.lock().expect("clicks lock");
        clicks.insert(id, t);
        while clicks.len() > KEEP_CLICKS {
            let Some(oldest) = clicks.keys().min().copied() else {
                break;
            };
            clicks.remove(&oldest);
        }
    }
    let shown = app
        .notification()
        .builder()
        .id(id)
        .title(title)
        .body(body_for_server(body))
        .show();
    if let Err(e) = shown {
        eprintln!("tmpmail-gui: could not show a notification: {e}");
    }
}

/// Whether new mail should be announced: the setting is on and the window
/// isn't focused (a hidden or minimized window never is).
pub fn wanted(shared: &Shared) -> bool {
    shared.gui.lock().expect("gui lock").notifications && !shared.focused.load(Ordering::Relaxed)
}

/// Brings the window back (from the tray, minimized or behind others) and
/// tells the page what to open.
pub fn reveal(app: &AppHandle, target: Option<Target>) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
    if let Some(t) = target {
        let _ = app.emit("reveal", json!({ "address": t.address, "id": t.id }));
    }
}

/// Routes clicks on notifications. Registered before the first notification
/// is shown: the plugin reports clicks only for notifications shown while a
/// handler exists.
pub fn register(app: &AppHandle, shared: Arc<Shared>) {
    let handle = app.clone();
    let registered = app.notification().on_action(move |performed| {
        // `tap` is a click on the notification itself; it has no buttons.
        if performed.action_id() != "tap" && performed.action_id() != "default" {
            return;
        }
        let target = performed.notification().and_then(|n| {
            shared
                .clicks
                .lock()
                .expect("clicks lock")
                .get(&n.id())
                .cloned()
        });
        reveal(&handle, target);
    });
    if let Err(e) = registered {
        eprintln!("tmpmail-gui: notification clicks won't open the app: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str, from: &str, subject: &str, intro: &str) -> Summary {
        Summary {
            id: id.into(),
            from: from.into(),
            subject: subject.into(),
            intro: intro.into(),
            ..Default::default()
        }
    }

    #[test]
    fn only_mail_after_the_first_listing_is_new() {
        let mut a = Arrivals::default();
        let one = summary("1", "A <a@x.com>", "hi", "");
        assert!(a.listed("me@x.com", std::slice::from_ref(&one)).is_empty());
        assert!(a.listed("me@x.com", std::slice::from_ref(&one)).is_empty());
        let two = summary(
            "2",
            "GitHub <noreply@github.com>",
            "Your code",
            "Use 482913 to sign in",
        );
        let new = a.listed("me@x.com", &[two, one]);
        assert_eq!(new.len(), 1);
        assert_eq!(new[0].id, "2");
        assert_eq!(new[0].sender, "GitHub");
        assert_eq!(new[0].code.as_deref(), Some("482913"));
        a.retain(|_| false);
        assert!(a.known.is_empty());
    }

    #[test]
    fn bursts_wait_for_the_group_window() {
        let mut a = Arrivals::default();
        assert!(a.due().is_none());
        a.queue(vec![arrival(
            "me@x.com",
            &summary("1", "a@x.com", "one", ""),
        )]);
        a.queue(vec![arrival(
            "me@x.com",
            &summary("2", "b@x.com", "two", ""),
        )]);
        assert!(a.due().is_none(), "the burst is still open");
        a.since = Some(Instant::now() - GROUP);
        let batch = a.due().unwrap();
        assert_eq!(
            batch.iter().map(|b| b.id.as_str()).collect::<Vec<_>>(),
            ["2", "1"]
        );
        assert!(a.due().is_none());
    }

    #[test]
    fn text_names_the_code_sender_and_subject() {
        let a = |id: &str, from: &str, subject: &str, intro: &str| {
            arrival("me@x.com", &summary(id, from, subject, intro))
        };
        let code = a(
            "1",
            "GitHub <noreply@github.com>",
            "Sign in",
            "Your code is 482913",
        );
        assert_eq!(
            text(std::slice::from_ref(&code)),
            ("Code 482913 from GitHub".into(), "Sign in".into())
        );
        let plain = a("2", "shop@x.com", "", "Thanks");
        assert_eq!(
            text(std::slice::from_ref(&plain)),
            ("shop@x.com".into(), "(no subject)".into())
        );
        let batch = [plain.clone(), code.clone(), plain.clone(), plain];
        let (title, body) = text(&batch);
        assert_eq!(title, "Code 482913 from GitHub, and 3 more");
        assert_eq!(body.lines().count(), 4);
        assert!(body.ends_with("and 1 more"));
        assert_eq!(target(&batch).unwrap().id.as_deref(), Some("1"));
    }

    #[test]
    fn sender_words_are_cleaned_and_capped() {
        let s = summary(
            "1",
            "\u{202e}Evil\u{1b}[31m <e@x.com>",
            &format!("{}\u{7}", "long ".repeat(40)),
            "",
        );
        let a = arrival("me@x.com", &s);
        assert!(!a.sender.contains('\u{202e}') && !a.sender.contains('\u{1b}'));
        assert!(a.subject.chars().count() <= SUBJECT_MAX);
        assert!(a.subject.ends_with('…'));
        assert!(!a.subject.contains('\u{7}'));
        assert_eq!(
            escape_markup("<img src=x> & <b>"),
            "&lt;img src=x&gt; &amp; &lt;b&gt;"
        );
    }
}
