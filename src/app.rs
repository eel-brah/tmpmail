use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::mpsc::Sender;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::config::Config;
use crate::error::{Error, Result};
use crate::provider::{self, CreateOpts, Inbox, Provider, Summary};
use crate::render::{clean, clean_line};
use crate::store::{State, Store};

// Used by the CLI's commands (`commands.rs` takes these through `super::*`).
#[cfg(feature = "cli")]
use {
    crate::extract,
    crate::provider::{Capabilities, Message},
    crate::push::Poller,
    crate::render::{body_text, safe_filename, safe_html_document},
    crate::system,
    regex::{Regex, RegexBuilder},
    std::sync::Arc,
    std::sync::atomic::{AtomicBool, Ordering},
};

#[cfg(feature = "cli")]
mod commands;

/// Options every command shares: the CLI's global flags.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "cli", derive(clap::Args))]
pub struct Global {
    /// Machine-readable JSON output
    #[cfg_attr(feature = "cli", arg(long, global = true))]
    pub json: bool,

    /// Inbox to use (address, local part or label); defaults to the active one
    #[cfg_attr(
        feature = "cli",
        arg(long, short = 'i', global = true, env = "TMPMAIL_INBOX")
    )]
    pub inbox: Option<String>,

    /// Disable colors (also honours NO_COLOR)
    #[cfg_attr(feature = "cli", arg(long, global = true))]
    pub no_color: bool,

    /// Only print essential output
    #[cfg_attr(feature = "cli", arg(long, short, global = true))]
    pub quiet: bool,

    /// Browser command for HTML and links (default: config `browser`, $BROWSER, then the system opener)
    #[cfg_attr(feature = "cli", arg(long, short = 'b', global = true))]
    pub browser: Option<String>,

    /// Clipboard command (default: config `clipboard_cmd`, else auto-detected)
    #[cfg_attr(feature = "cli", arg(long, global = true))]
    pub clipboard_cmd: Option<String>,
}

pub struct App {
    pub global: Global,
    pub config: Config,
    pub store: Store,
    pub state: State,
    color: bool,
    /// Built once per run so providers can reuse what they fetched.
    providers: HashMap<String, Box<dyn Provider>>,
    /// Where notes and warnings go instead of stderr (the TUI owns the screen).
    log: Option<Sender<String>>,
    /// Set from outside (the MCP server) to stop a running wait.
    #[cfg(feature = "cli")]
    cancel: Option<Arc<AtomicBool>>,
}

/// What `--json` prints for an inbox. Credentials are never included.
#[derive(Serialize)]
pub struct InboxView<'a> {
    address: &'a str,
    provider: &'a str,
    label: Option<&'a str>,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    expired: bool,
    active: bool,
    private: bool,
}

impl App {
    pub fn new(global: Global) -> Result<Self> {
        let config = Config::load()?;
        crate::http::set_timeout(config.http_timeout);
        let store = Store::new(Store::default_dir());
        let state = store.load()?;
        let color = !global.no_color
            && !global.json
            && std::env::var_os("NO_COLOR").is_none()
            && std::io::stdout().is_terminal();
        Ok(App {
            global,
            config,
            store,
            state,
            color,
            providers: HashMap::new(),
            log: None,
            #[cfg(feature = "cli")]
            cancel: None,
        })
    }

    /// An app with the default config and an empty state kept in `dir`,
    /// for tests that must not read the user's files.
    #[cfg(test)]
    pub(crate) fn for_test(dir: std::path::PathBuf) -> Self {
        App {
            global: Global::default(),
            config: Config::default(),
            store: Store::new(dir),
            state: State::default(),
            color: false,
            providers: HashMap::new(),
            log: None,
            #[cfg(feature = "cli")]
            cancel: None,
        }
    }

    /// Uses `p` for its provider id instead of the built-in one.
    #[cfg(test)]
    pub(crate) fn set_provider(&mut self, p: Box<dyn Provider>) {
        self.providers.insert(p.id().to_string(), p);
    }

    /// Sends notes and warnings to `log` instead of printing them.
    pub fn log_to(&mut self, log: Sender<String>) {
        self.log = Some(log);
    }

    #[cfg(feature = "cli")]
    /// Makes waits give up with `Error::Cancelled` once `flag` is set.
    pub(crate) fn cancel_on(&mut self, flag: Arc<AtomicBool>) {
        self.cancel = Some(flag);
    }

    #[cfg(feature = "cli")]
    fn check_cancel(&self) -> Result<()> {
        match &self.cancel {
            Some(c) if c.load(Ordering::Relaxed) => Err(Error::Cancelled),
            _ => Ok(()),
        }
    }

    // ---------- output helpers ----------

    /// Wraps `s` in an SGR color code when colors are on. Pad before
    /// painting: the escape codes would count toward the width.
    fn paint(&self, code: &str, s: &str) -> String {
        if self.color && !s.trim().is_empty() {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    #[cfg(feature = "cli")]
    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }

    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }

    #[cfg(feature = "cli")]
    fn green(&self, s: &str) -> String {
        self.paint("32", s)
    }

    #[cfg(feature = "cli")]
    fn red(&self, s: &str) -> String {
        self.paint("31", s)
    }

    #[cfg(feature = "cli")]
    fn yellow(&self, s: &str) -> String {
        self.paint("33", s)
    }

    #[cfg(feature = "cli")]
    fn cyan(&self, s: &str) -> String {
        self.paint("36", s)
    }

    #[cfg(feature = "cli")]
    fn json_out(&self, v: &impl Serialize) {
        println!("{}", serde_json::to_string_pretty(v).expect("serializable"));
    }

    /// Messages can quote provider responses, so they are cleaned like mail.
    fn note(&self, msg: &str) {
        if let Some(log) = &self.log {
            let _ = log.send(clean_line(msg));
        } else if !self.global.quiet && !self.global.json {
            eprintln!("{}", self.dim(&clean(msg)));
        }
    }

    fn warn(&self, msg: &str) {
        if let Some(log) = &self.log {
            let _ = log.send(clean_line(msg));
        } else {
            eprintln!("tmpmail: {}", clean_line(msg));
        }
    }

    pub fn view<'a>(&self, inbox: &'a Inbox) -> InboxView<'a> {
        InboxView {
            address: &inbox.address,
            provider: &inbox.provider,
            label: inbox.label.as_deref(),
            created_at: inbox.created_at,
            expires_at: inbox.expires_at,
            expired: inbox.is_expired(),
            active: self.state.active.as_deref() == Some(inbox.address.as_str()),
            private: provider::get(&inbox.provider)
                .map(|p| p.caps().private)
                .unwrap_or(true),
        }
    }

    // ---------- inbox plumbing ----------

    #[cfg(feature = "cli")]
    /// Runs `f` against the selected inbox, then persists any credential
    /// refresh the provider made.
    fn with_inbox<T>(
        &mut self,
        f: impl FnOnce(&dyn Provider, &mut Inbox) -> Result<T>,
    ) -> Result<T> {
        let n = self.state.resolve(self.global.inbox.as_deref())?;
        self.with_inbox_at(n, f)
    }

    pub fn with_inbox_at<T>(
        &mut self,
        n: usize,
        f: impl FnOnce(&dyn Provider, &mut Inbox) -> Result<T>,
    ) -> Result<T> {
        let mut inbox = self.state.inboxes[n].clone();
        let p = self.provider(&inbox.provider)?;
        let result = f(p, &mut inbox);
        if inbox != self.state.inboxes[n] {
            self.state.inboxes[n] = inbox.clone();
            // Merge into the file as it is now: another tmpmail may have
            // changed other inboxes. One dropped meanwhile stays dropped.
            self.store.update(|s| {
                if let Some(k) = s.position(&inbox.address) {
                    s.inboxes[k] = inbox;
                }
            })?;
        }
        result
    }

    fn provider(&mut self, id: &str) -> Result<&dyn Provider> {
        if !self.providers.contains_key(id) {
            self.providers.insert(id.to_string(), provider::get(id)?);
        }
        Ok(self.providers[id].as_ref())
    }

    #[cfg(feature = "cli")]
    /// Inbox indexes a command should poll: all stored ones with
    /// `--all-inboxes`, otherwise just the selected one.
    fn targets(&self, all: bool) -> Result<Vec<usize>> {
        if !all {
            return Ok(vec![self.state.resolve(self.global.inbox.as_deref())?]);
        }
        if self.global.inbox.is_some() {
            return Err(Error::Usage(
                "--all-inboxes and --inbox can't be combined".into(),
            ));
        }
        if self.state.inboxes.is_empty() {
            return Err(Error::NoInbox);
        }
        Ok((0..self.state.inboxes.len()).collect())
    }

    #[cfg(feature = "cli")]
    fn current(&self) -> Result<&Inbox> {
        let n = self.state.resolve(self.global.inbox.as_deref())?;
        Ok(&self.state.inboxes[n])
    }

    pub fn sorted_list(p: &dyn Provider, inbox: &mut Inbox) -> Result<Vec<Summary>> {
        let mut list = p.list(inbox)?;
        list.sort_by_key(|s| std::cmp::Reverse(s.date));
        Ok(list)
    }

    #[cfg(feature = "cli")]
    /// Accepts an exact id, a list number from `ls`, or a unique id prefix.
    pub(crate) fn resolve_id(
        p: &dyn Provider,
        inbox: &mut Inbox,
        id: Option<&str>,
    ) -> Result<String> {
        let list = Self::sorted_list(p, inbox)?;
        let Some(id) = id else {
            return list
                .first()
                .map(|s| s.id.clone())
                .ok_or_else(|| Error::NotFound(format!("no messages in {} yet", inbox.address)));
        };
        // A list number first: Guerrilla's welcome mail has the id "1", and
        // `read 1` must still mean the newest message.
        if let Ok(n) = id.parse::<usize>() {
            if (1..=list.len()).contains(&n) {
                return Ok(list[n - 1].id.clone());
            }
        }
        if list.iter().any(|s| s.id == id) {
            return Ok(id.to_string());
        }
        let hits: Vec<&Summary> = list.iter().filter(|s| s.id.starts_with(id)).collect();
        match hits.as_slice() {
            [one] => Ok(one.id.clone()),
            [] => Err(Error::NotFound(format!(
                "no message {id:?} in {}",
                inbox.address
            ))),
            _ => Err(Error::Usage(format!(
                "{id:?} matches several messages, use more characters"
            ))),
        }
    }

    #[cfg(feature = "cli")]
    /// Runs `f` on the message `id` resolves to in the selected inbox.
    fn with_message<T>(
        &mut self,
        id: Option<&str>,
        f: impl FnOnce(&dyn Provider, &mut Inbox, &str) -> Result<T>,
    ) -> Result<T> {
        self.with_inbox(|p, inbox| {
            let id = Self::resolve_id(p, inbox, id)?;
            f(p, inbox, &id)
        })
    }

    /// Creates an inbox with `explicit`, or else the first configured
    /// provider (other than `exclude`) that supports `opts` and works.
    pub fn create_with_fallback(
        &self,
        explicit: Option<&str>,
        exclude: Option<&str>,
        opts: &CreateOpts,
    ) -> Result<Inbox> {
        if let Some(id) = explicit {
            let p = provider::get(id)?;
            let inbox = checked_address(p.as_ref(), p.create(opts)?)?;
            self.note_unused_password(p.as_ref(), opts);
            return Ok(inbox);
        }
        let mut candidates = Vec::new();
        for id in self.config.provider_order() {
            if exclude == Some(id.as_str()) {
                continue;
            }
            let p = provider::get(&id)?;
            let caps = p.caps();
            if opts.user.is_some() && !caps.custom_user
                || opts.domain.is_some() && !caps.choose_domain
            {
                continue;
            }
            candidates.push(p);
        }
        let (mut failures, mut unavailable) = (Vec::new(), Vec::new());
        if let Some(domain) = &opts.domain {
            candidates = order_by_domain(candidates, domain, &mut unavailable);
        }
        // A password (maybe just TMPMAIL_PASSWORD in the environment) only
        // matters to providers with accounts: try those first, not only those.
        candidates.sort_by_key(|p| opts.password.is_some() && !p.caps().login);

        for p in candidates {
            match p.create(opts) {
                Ok(inbox) => {
                    let inbox = checked_address(p.as_ref(), inbox)?;
                    if !failures.is_empty() {
                        self.note(&format!("using {} instead", p.name()));
                    }
                    self.note_unused_password(p.as_ref(), opts);
                    return Ok(inbox);
                }
                Err(e) if e.is_provider_failure() => {
                    self.warn(&format!("{e}, trying the next provider"));
                    failures.push(e.to_string());
                }
                // A requested domain may belong to a later provider.
                Err(e @ Error::DomainUnavailable { .. }) => unavailable.push(e.to_string()),
                Err(e) => return Err(e),
            }
        }
        if !failures.is_empty() {
            failures.extend(unavailable);
            return Err(Error::Unreachable {
                provider: "all providers".into(),
                detail: failures.join("; "),
            });
        }
        // Every provider was up but none offers that domain: a usage error.
        if !unavailable.is_empty() {
            return Err(Error::Usage(unavailable.join("; ")));
        }
        Err(Error::Usage(
            "no configured provider supports these options (see `tmpmail providers`)".into(),
        ))
    }

    fn note_unused_password(&self, p: &dyn Provider, opts: &CreateOpts) {
        if opts.password.is_some() && !p.caps().login {
            self.note(&format!(
                "{} has no accounts, the password is not used",
                p.name()
            ));
        }
    }

    #[cfg(feature = "cli")]
    pub(crate) fn clipboard_cmd(&self) -> Option<String> {
        self.global
            .clipboard_cmd
            .clone()
            .or_else(|| self.config.clipboard_cmd.clone())
    }

    #[cfg(feature = "cli")]
    pub(crate) fn browser(&self) -> Option<String> {
        self.global
            .browser
            .clone()
            .or_else(|| self.config.browser.clone())
    }

    #[cfg(feature = "cli")]
    /// Creates a fresh inbox to replace the selected one, with the same
    /// provider if it works, and drops the old one unless `keep`. The caller
    /// stores the new inbox.
    pub(crate) fn replace_current(&mut self, keep: bool) -> Result<Inbox> {
        let old = self.current()?.clone();
        // Prefer the same provider, fall back like `new` does.
        let opts = CreateOpts::default();
        let p = provider::get(&old.provider)?;
        let mut inbox = match p.create(&opts).and_then(|i| checked_address(p.as_ref(), i)) {
            Ok(i) => i,
            Err(e) if e.is_provider_failure() => {
                self.warn(&format!("{e}, trying other providers"));
                self.create_with_fallback(None, Some(&old.provider), &opts)?
            }
            Err(e) => return Err(e),
        };
        inbox.label = old.label.clone();
        if !keep {
            let _ = p.delete_inbox(&mut old.clone());
            self.forget(&old.address)?;
        }
        Ok(inbox)
    }

    /// Removes a stored inbox (by exact address) from the state file.
    pub fn forget(&mut self, address: &str) -> Result<()> {
        self.state = self
            .store
            .update(|s| {
                if let Some(k) = s.position(address) {
                    s.remove(k);
                }
            })?
            .0;
        Ok(())
    }
}

/// Asks every candidate for its domains at once and keeps those offering
/// `domain` first, then those that couldn't answer (they may be down). The
/// rest are left out and noted in `unavailable`, so `new -d` doesn't try each
/// provider in turn.
fn order_by_domain(
    candidates: Vec<Box<dyn Provider>>,
    domain: &str,
    unavailable: &mut Vec<String>,
) -> Vec<Box<dyn Provider>> {
    let lists: Vec<Result<Vec<String>>> = std::thread::scope(|s| {
        let lookups: Vec<_> = candidates
            .iter()
            .map(|p| {
                let id = p.id();
                s.spawn(move || provider::get(id)?.domains())
            })
            .collect();
        lookups
            .into_iter()
            .map(|l| l.join().expect("domain lookup panicked"))
            .collect()
    });
    let (mut offering, mut unknown) = (Vec::new(), Vec::new());
    for (p, list) in candidates.into_iter().zip(lists) {
        match list {
            Ok(l) if l.iter().any(|d| d == domain) => offering.push(p),
            Ok(l) => unavailable.push(provider::domain_unavailable(p.id(), domain, &l).to_string()),
            Err(_) => unknown.push(p),
        }
    }
    offering.extend(unknown);
    offering
}

/// Rejects an address a provider returned that couldn't be a real one, such
/// as one carrying terminal escapes: it is printed and stored as is.
pub(crate) fn checked_address(p: &dyn Provider, inbox: Inbox) -> Result<Inbox> {
    let a = &inbox.address;
    // `clean_line` drops control and bidi characters and folds whitespace.
    let plausible = a.split('@').count() == 2
        && !a.starts_with('@')
        && !a.ends_with('@')
        && !a.contains(char::is_whitespace)
        && clean_line(a) == *a;
    if plausible {
        Ok(inbox)
    } else {
        Err(Error::BadResponse {
            provider: p.id().to_string(),
            detail: format!("invalid address {:?}", clean_line(a)),
            status: None,
        })
    }
}

#[cfg(feature = "cli")]
pub(crate) fn compile(pattern: &str) -> Result<Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .map_err(|e| Error::Usage(format!("invalid regex {pattern:?}: {e}")))
}

#[cfg(feature = "cli")]
pub(crate) fn compile_plain(pattern: &str) -> Result<Regex> {
    Regex::new(pattern).map_err(|e| Error::Usage(format!("invalid regex {pattern:?}: {e}")))
}

#[cfg(feature = "cli")]
pub(crate) fn message_body(m: &Message) -> String {
    body_text(m.text.as_deref(), m.html.as_deref(), 200)
}

#[cfg(feature = "cli")]
pub(crate) fn message_links(m: &Message) -> Vec<String> {
    extract::links(
        m.text.as_deref().unwrap_or_default(),
        m.html.as_deref().unwrap_or_default(),
    )
}

#[cfg(feature = "cli")]
pub(crate) fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(feature = "cli")]
pub(crate) fn relative(t: DateTime<Utc>) -> String {
    let secs = (Utc::now() - t).num_seconds();
    let (future, secs) = (secs < 0, secs.unsigned_abs());
    let text = match secs {
        0..=59 => {
            return if future {
                "in <1m".into()
            } else {
                "just now".into()
            };
        }
        60..=3599 => format!("{}m", secs / 60),
        3600..=86399 => format!("{}h", secs / 3600),
        _ => format!("{}d", secs / 86400),
    };
    if future {
        format!("in {text}")
    } else {
        format!("{text} ago")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[cfg(feature = "cli")]
    #[test]
    fn relative_times() {
        let now = Utc::now();
        assert_eq!(relative(now), "just now");
        assert_eq!(relative(now - chrono::Duration::minutes(5)), "5m ago");
        assert_eq!(
            relative(now + chrono::Duration::minutes(30) + chrono::Duration::seconds(5)),
            "in 30m"
        );
        assert_eq!(relative(now - chrono::Duration::days(3)), "3d ago");
    }

    #[test]
    fn provider_addresses_are_validated() {
        let p = provider::get("mailtm").unwrap();
        let ok = |a: &str| {
            checked_address(p.as_ref(), Inbox::new("mailtm", a.into(), Value::Null)).is_ok()
        };
        assert!(ok("otter1@maxxspace.com"));
        assert!(!ok("x\u{1b}]52;c;AAAA\u{7}@evil.com"));
        assert!(!ok("a\u{202e}b@x.com"));
        assert!(!ok("no-at-sign"));
        assert!(!ok("a b@x.com"));
    }

    #[cfg(feature = "cli")]
    #[test]
    fn truncation() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
    }
}
