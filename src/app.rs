use std::collections::{HashMap, HashSet};
use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use serde_json::{Value, json};

use crate::cli::*;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::extract;
use crate::provider::{self, Capabilities, CreateOpts, Inbox, Message, Provider, Summary};
use crate::push::Poller;
use crate::render::{body_text, clean, clean_line, safe_filename, safe_html_document};
use crate::store::{State, Store};
use crate::system;

pub struct App {
    pub global: Global,
    pub config: Config,
    pub store: Store,
    pub state: State,
    color: bool,
    /// Built once per run so providers can reuse what they fetched.
    providers: HashMap<String, Box<dyn Provider>>,
}

/// What `--json` prints for an inbox. Credentials are never included.
#[derive(Serialize)]
struct InboxView<'a> {
    address: &'a str,
    provider: &'a str,
    label: Option<&'a str>,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    expired: bool,
    active: bool,
    private: bool,
}

/// One line of `tmpmail providers`.
#[derive(Serialize)]
struct ProviderRow {
    id: &'static str,
    name: &'static str,
    website: &'static str,
    in_fallback_order: Option<usize>,
    capabilities: Capabilities,
    status: Option<Health>,
}

#[derive(Serialize)]
struct Health {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

impl App {
    pub fn new(global: Global) -> Result<Self> {
        let config = Config::load()?;
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
        })
    }

    pub fn run(&mut self, command: Command) -> Result<()> {
        match command {
            Command::New(a) => self.new_inbox(a),
            Command::Ls(a) => self.ls(a),
            Command::Read(a) => self.read(a),
            Command::Wait(a) => self.wait_cmd(a),
            Command::Otp(a) => self.otp(a),
            Command::Links(a) => self.links(a),
            Command::Watch(a) => self.watch(a),
            Command::Attachments(a) => self.attachments(a),
            Command::Save(a) => self.save(a),
            Command::Delete(a) => self.delete(a),
            Command::Whoami => self.whoami(),
            Command::Copy => self.copy(),
            Command::Qr => self.qr(),
            Command::Domains(a) => self.domains(a),
            Command::Inboxes(a) => self.inboxes(a),
            Command::Use { address } => self.use_inbox(&address),
            Command::Login(a) => self.login(a),
            Command::Drop(a) => self.drop_inbox(a),
            Command::Rotate(a) => self.rotate(a),
            Command::Providers(a) => self.providers(a),
            Command::Completions { .. } | Command::Man { .. } => unreachable!("handled in main"),
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

    fn bold(&self, s: &str) -> String {
        self.paint("1", s)
    }

    fn dim(&self, s: &str) -> String {
        self.paint("2", s)
    }

    fn green(&self, s: &str) -> String {
        self.paint("32", s)
    }

    fn red(&self, s: &str) -> String {
        self.paint("31", s)
    }

    fn yellow(&self, s: &str) -> String {
        self.paint("33", s)
    }

    fn cyan(&self, s: &str) -> String {
        self.paint("36", s)
    }

    fn json_out(&self, v: &impl Serialize) {
        println!("{}", serde_json::to_string_pretty(v).expect("serializable"));
    }

    /// Messages can quote provider responses, so they are cleaned like mail.
    fn note(&self, msg: &str) {
        if !self.global.quiet && !self.global.json {
            eprintln!("{}", self.dim(&clean(msg)));
        }
    }

    fn warn(&self, msg: &str) {
        eprintln!("tmpmail: {}", clean_line(msg));
    }

    fn view<'a>(&self, inbox: &'a Inbox) -> InboxView<'a> {
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

    /// Runs `f` against the selected inbox, then persists any credential
    /// refresh the provider made.
    fn with_inbox<T>(
        &mut self,
        f: impl FnOnce(&dyn Provider, &mut Inbox) -> Result<T>,
    ) -> Result<T> {
        let n = self.state.resolve(self.global.inbox.as_deref())?;
        self.with_inbox_at(n, f)
    }

    fn with_inbox_at<T>(
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

    fn current(&self) -> Result<&Inbox> {
        let n = self.state.resolve(self.global.inbox.as_deref())?;
        Ok(&self.state.inboxes[n])
    }

    fn sorted_list(p: &dyn Provider, inbox: &mut Inbox) -> Result<Vec<Summary>> {
        let mut list = p.list(inbox)?;
        list.sort_by_key(|s| std::cmp::Reverse(s.date));
        Ok(list)
    }

    /// Accepts an exact id, a list number from `ls`, or a unique id prefix.
    fn resolve_id(p: &dyn Provider, inbox: &mut Inbox, id: Option<&str>) -> Result<String> {
        let list = Self::sorted_list(p, inbox)?;
        let Some(id) = id else {
            return list
                .first()
                .map(|s| s.id.clone())
                .ok_or_else(|| Error::NotFound(format!("no messages in {} yet", inbox.address)));
        };
        if list.iter().any(|s| s.id == id) {
            return Ok(id.to_string());
        }
        if let Ok(n) = id.parse::<usize>() {
            if (1..=list.len()).contains(&n) {
                return Ok(list[n - 1].id.clone());
            }
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

    fn fetch(&mut self, msg: &MessageRef) -> Result<Message> {
        self.with_message(msg.id.as_deref(), |p, inbox, id| p.get(inbox, id))
    }

    /// Stores `inbox` as the active one and prints it.
    fn activate(&mut self, inbox: &Inbox, copy: bool) -> Result<()> {
        let added = inbox.clone();
        self.state = self.store.update(|s| s.add(added))?.0;
        if copy {
            system::copy(&inbox.address, self.clipboard_cmd().as_deref())?;
        }
        if self.global.json {
            self.json_out(&self.view(inbox));
        } else {
            println!("{}", clean_line(&inbox.address));
        }
        Ok(())
    }

    // ---------- commands ----------

    fn new_inbox(&mut self, a: NewArgs) -> Result<()> {
        let (mut user, mut domain) = (a.user, a.domain);
        if let Some(addr) = &a.address {
            let (u, d) = addr
                .split_once('@')
                .ok_or_else(|| Error::Usage(format!("{addr:?} is not an email address")))?;
            user = Some(u.to_string());
            domain = Some(d.to_string());
        }
        let opts = CreateOpts {
            user,
            domain,
            password: a.password,
        };
        let mut inbox = self.create_with_fallback(a.provider.as_deref(), None, &opts)?;
        inbox.label = a.label;
        let p = provider::get(&inbox.provider)?;
        self.activate(&inbox, a.copy)?;
        if !self.global.json {
            if !p.caps().private {
                self.warn(&format!(
                    "warning: {} inboxes are public, anyone who knows this address can read its mail",
                    p.name()
                ));
            }
            let mut info = format!("via {} ({})", p.name(), p.caps().lifetime);
            if a.copy {
                info.push_str(", copied to clipboard");
            }
            self.note(&info);
        }
        Ok(())
    }

    /// Creates an inbox with `explicit`, or else the first configured
    /// provider (other than `exclude`) that supports `opts` and works.
    fn create_with_fallback(
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

    fn clipboard_cmd(&self) -> Option<String> {
        self.global
            .clipboard_cmd
            .clone()
            .or_else(|| self.config.clipboard_cmd.clone())
    }

    fn browser(&self) -> Option<String> {
        self.global
            .browser
            .clone()
            .or_else(|| self.config.browser.clone())
    }

    fn ls(&mut self, a: LsArgs) -> Result<()> {
        let targets = self.targets(a.all_inboxes)?;
        let mut sections = Vec::new();
        for n in targets {
            let address = self.state.inboxes[n].address.clone();
            let created = self.state.inboxes[n].created_at;
            let result = self.with_inbox_at(n, |p, inbox| Self::sorted_list(p, inbox));
            match result {
                Ok(list) => sections.push((address, created, Ok(list))),
                // With -a, one dead inbox must not hide the others.
                Err(e) if a.all_inboxes => sections.push((address, created, Err(e))),
                Err(e) => return Err(e),
            }
        }

        if self.global.json {
            if !a.all_inboxes {
                let (address, created, list) = sections.remove(0);
                self.json_out(&json!({
                    "address": address,
                    "created_at": created,
                    "messages": list?,
                }));
            } else {
                let all: Vec<Value> = sections
                    .iter()
                    .map(|(address, created, r)| match r {
                        Ok(list) => json!({
                            "address": address,
                            "created_at": created,
                            "messages": list,
                        }),
                        Err(e) => json!({
                            "address": address,
                            "created_at": created,
                            "messages": [],
                            "error": { "kind": e.kind(), "message": e.to_string() },
                        }),
                    })
                    .collect();
                self.json_out(&json!({ "inboxes": all }));
            }
            return Ok(());
        }

        let total = sections.len();
        for (k, (address, created, result)) in sections.into_iter().enumerate() {
            if k > 0 {
                println!();
            }
            if !self.global.quiet || total > 1 {
                let since = format!(
                    "created {} ({})",
                    created
                        .with_timezone(&chrono::Local)
                        .format("%Y-%m-%d %H:%M"),
                    relative(created),
                );
                println!(
                    "{}  {}",
                    self.bold(&format!("[ Inbox for {} ]", clean_line(&address))),
                    self.dim(&since),
                );
            }
            match result {
                Ok(list) if list.is_empty() => {
                    if !self.global.quiet {
                        println!("No messages yet. `tmpmail wait` blocks until one arrives.");
                    }
                }
                Ok(list) => self.print_list(&list),
                Err(e) => println!(
                    "{}",
                    self.dim(&format!("  unavailable: {}", clean_line(&e.to_string())))
                ),
            }
        }
        if total > 1 && !self.global.quiet {
            self.note("\nread one with: tmpmail -i ADDRESS read N");
        }
        Ok(())
    }

    fn print_list(&self, list: &[Summary]) {
        let width = term_width();
        for (n, s) in list.iter().enumerate() {
            let att = if s.has_attachments { " [att]" } else { "" };
            let when = s.date.map(relative).unwrap_or_default();
            let from = truncate(&clean_line(&s.from), 28);
            let room = width.saturating_sub(4 + 10 + 30 + att.len()).max(20);
            let subject = truncate(&clean_line(&s.subject), room);
            // Unread mail stands out; read mail is plain.
            let (marker, subject) = if s.seen {
                (" ".to_string(), subject)
            } else {
                (self.cyan("*"), self.bold(&subject))
            };
            println!(
                "{:>3}{marker} {}  {from:<28}  {subject}{}",
                n + 1,
                self.dim(&format!("{when:>8}")),
                self.yellow(att),
            );
        }
    }

    fn read(&mut self, a: ReadArgs) -> Result<()> {
        if a.raw {
            let raw = self.with_message(a.msg.id.as_deref(), |p, inbox, id| p.raw(inbox, id))?;
            let mut out = std::io::stdout();
            // Exact bytes when piped; a sender's escape sequences must not
            // reach a terminal, though.
            if out.is_terminal() {
                write!(out, "{}", clean(&String::from_utf8_lossy(&raw)))
            } else {
                out.write_all(&raw)
            }
            .map_err(anyhow::Error::from)?;
            return Ok(());
        }
        let msg = self.fetch(&a.msg)?;
        if a.html {
            return self.open_html(&msg, a.images);
        }
        if self.global.json {
            self.json_out(&msg);
            return Ok(());
        }
        let text = self.render_message(&msg);
        let rows = system::term_size().map_or(0, |(_, rows)| rows);
        // Leave a row for the shell prompt.
        let fits = rows == 0 || text.lines().count() < rows;
        if a.no_pager || fits || !std::io::stdout().is_terminal() {
            print!("{text}");
        } else if let Err(e) = system::page(&text, self.config.pager.as_deref()) {
            self.warn(&format!("pager failed ({e:#}), printing instead"));
            print!("{text}");
        }
        self.note_html_version(&msg);
        Ok(())
    }

    fn print_message(&self, msg: &Message) {
        if self.global.json {
            self.json_out(msg);
            return;
        }
        print!("{}", self.render_message(msg));
        self.note_html_version(msg);
    }

    fn note_html_version(&self, msg: &Message) {
        if msg.html.is_some() && !self.global.quiet {
            // The note goes to stderr; keep it after the body when piped.
            let _ = std::io::stdout().flush();
            self.note(&format!("\n(HTML version: tmpmail read {} --html)", msg.id));
        }
    }

    /// Headers and text body, ready to print or page.
    fn render_message(&self, msg: &Message) -> String {
        let width = term_width().min(100);
        let mut out = String::new();
        let mut header = |k: &str, v: String| {
            out.push_str(&format!("{} {v}\n", self.bold(&format!("{k:<8}"))));
        };
        header("From:", clean_line(&msg.from));
        header("To:", clean_line(&msg.to.join(", ")));
        header("Subject:", self.bold(&clean_line(&msg.subject)));
        if let Some(d) = msg.date {
            let local = d.with_timezone(&chrono::Local);
            header(
                "Date:",
                format!("{} ({})", local.format("%Y-%m-%d %H:%M:%S"), relative(d)),
            );
        }
        if !msg.attachments.is_empty() {
            let names: Vec<String> = msg.attachments.iter().map(|a| a.filename.clone()).collect();
            header(
                "Attach:",
                format!(
                    "{} {}",
                    self.yellow(&clean_line(&names.join(", "))),
                    self.dim(&format!("(tmpmail save {})", clean_line(&msg.id)))
                ),
            );
        }
        out.push('\n');
        let body = body_text(msg.text.as_deref(), msg.html.as_deref(), width);
        out.push_str(&clean(body.trim_end()));
        out.push('\n');
        out
    }

    fn open_html(&self, msg: &Message, images: bool) -> Result<()> {
        let html = match (&msg.html, &msg.text) {
            (Some(h), _) => h.clone(),
            (None, Some(t)) => format!("<pre>{}</pre>", crate::render::escape_html(t)),
            (None, None) => String::new(),
        };
        let doc = safe_html_document(&msg.subject, &html, images);
        let name = format!("{}.html", safe_filename(&msg.id, "message"));
        let path = system::write_view_file(self.store.dir(), &name, &doc)?;
        system::open(&path.to_string_lossy(), self.browser().as_deref())?;
        self.note(&format!("opened {}", path.display()));
        Ok(())
    }

    // ----- wait / otp / links -----

    fn wait_cmd(&mut self, a: WaitArgs) -> Result<()> {
        let otp_re = a.otp_regex.as_deref().map(compile_plain).transpose()?;
        let (want_otp, want_link) = (a.otp, a.link);
        // `Some(found)` accepts the message; `found` is the code or link asked for.
        let (n, msg, text, found) = self.wait_for(&a.filter, |m, body| {
            if want_otp {
                extract::otp(&m.subject, body, otp_re.as_ref()).map(Some)
            } else if want_link {
                message_links(m).into_iter().next().map(Some)
            } else {
                Some(None)
            }
        })?;

        let inbox = self.state.inboxes[n].address.clone();
        if let Some(code) = found.as_deref().filter(|_| want_otp) {
            self.print_code(code, &msg, &text, otp_re.is_some(), &inbox);
        } else if let Some(link) = found {
            if self.global.json {
                self.json_out(&json!({
                    "link": link,
                    "message_id": msg.id,
                    "inbox": inbox,
                }));
            } else {
                println!("{}", clean_line(&link));
            }
        } else if self.global.json {
            let mut v = serde_json::to_value(&msg).expect("serializable");
            v["inbox"] = json!(inbox);
            self.json_out(&v);
        } else {
            self.print_message(&msg);
        }
        if a.delete {
            let id = msg.id.clone();
            self.with_inbox_at(n, |p, inbox| p.delete_message(inbox, &id))?;
        }
        Ok(())
    }

    /// `body` is the message text the code came from. Heuristic candidates
    /// are listed only when no custom regex picked the code.
    fn print_code(&self, code: &str, msg: &Message, body: &str, custom: bool, inbox: &str) {
        if self.global.json {
            let candidates = if custom {
                Vec::new()
            } else {
                extract::otp_candidates(&msg.subject, body)
            };
            self.json_out(&json!({
                "code": code,
                "candidates": candidates,
                "inbox": inbox,
                "message_id": msg.id,
                "from": msg.from,
                "subject": msg.subject,
            }));
        } else {
            println!("{}", clean_line(code));
        }
    }

    /// Polls the target inbox(es) until a message passes the filter and
    /// `accept` (given the message and its body text) returns `Some`, or
    /// times out. Returns the inbox index, the message, its body text and what
    /// `accept` found.
    fn wait_for<T>(
        &mut self,
        f: &WaitFilter,
        accept: impl Fn(&Message, &str) -> Option<T>,
    ) -> Result<(usize, Message, String, T)> {
        let from = f.from.as_deref().map(compile).transpose()?;
        let subject = f.subject.as_deref().map(compile).transpose()?;
        let body = f.body.as_deref().map(compile).transpose()?;
        let timeout = f.timeout.unwrap_or_else(|| self.config.timeout());
        let interval = self.config.poll_interval(f.interval);
        let deadline = Instant::now() + Duration::from_secs(timeout);
        let all = f.all_inboxes;
        let mut targets = self.targets(all)?;

        // Keyed by (inbox, message id): ids are only unique per inbox.
        let mut seen: HashSet<(usize, String)> = HashSet::new();
        if !f.include_existing {
            for &n in &targets.clone() {
                match self.with_inbox_at(n, |p, inbox| p.list(inbox)) {
                    Ok(list) => seen.extend(list.into_iter().map(|s| (n, s.id))),
                    Err(e) if all => self.skip_or_warn(&mut targets, n, &e),
                    Err(e) => return Err(e),
                }
            }
        }
        if targets.is_empty() {
            return Err(Error::NotFound("no usable inbox to wait on".into()));
        }
        self.note(&format!(
            "waiting for mail to {} (timeout {timeout}s)...",
            self.targets_label(&targets)
        ));

        let mut failing: HashSet<usize> = HashSet::new();
        let mut poller = Poller::new(Duration::from_secs(interval), self.config.push());
        loop {
            for n in targets.clone() {
                if !poller.is_due(&self.state.inboxes[n].address) {
                    continue;
                }
                let Some(list) = self.poll(n, &mut targets, &mut failing, all, &mut poller)? else {
                    continue;
                };
                // Oldest first, so the first matching arrival wins.
                for s in list.iter().rev() {
                    let key = (n, s.id.clone());
                    if seen.contains(&key) {
                        continue;
                    }
                    if from.as_ref().is_some_and(|re| !re.is_match(&s.from))
                        || subject.as_ref().is_some_and(|re| !re.is_match(&s.subject))
                    {
                        seen.insert(key);
                        continue;
                    }
                    let msg = match self.with_inbox_at(n, |p, inbox| p.get(inbox, &key.1)) {
                        Ok(m) => m,
                        Err(e) if e.is_provider_failure() => break,
                        Err(e) => return Err(e),
                    };
                    seen.insert(key);
                    let text = message_body(&msg);
                    if body.as_ref().is_some_and(|re| !re.is_match(&text)) {
                        continue;
                    }
                    if let Some(found) = accept(&msg, &text) {
                        if all {
                            self.note(&format!("received by {}", self.state.inboxes[n].address));
                        }
                        return Ok((n, msg, text, found));
                    }
                }
            }
            if targets.is_empty() {
                return Err(Error::NotFound("all inboxes have expired".into()));
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout(timeout));
            }
            poller.sleep(Some(deadline));
        }
    }

    /// Lists inbox `n` for a polling loop and (re)starts its push stream. A
    /// temporary provider failure is warned about once and yields `None`; in
    /// multi-inbox mode other errors go through `skip_or_warn`.
    fn poll(
        &mut self,
        n: usize,
        targets: &mut Vec<usize>,
        failing: &mut HashSet<usize>,
        multi: bool,
        poller: &mut Poller,
    ) -> Result<Option<Vec<Summary>>> {
        let address = self.state.inboxes[n].address.clone();
        let result = self.with_inbox_at(n, |p, inbox| {
            let list = Self::sorted_list(p, inbox)?;
            poller.subscribe(p, inbox);
            Ok(list)
        });
        poller.polled(&address, result.is_ok());
        match result {
            Ok(list) => {
                if failing.remove(&n) {
                    self.note(&format!(
                        "{} reachable again",
                        self.state.inboxes[n].address
                    ));
                }
                Ok(Some(list))
            }
            Err(e) if e.is_provider_failure() => {
                if failing.insert(n) {
                    self.warn(&format!("{e}; will keep trying"));
                }
                Ok(None)
            }
            Err(e) if multi => {
                self.skip_or_warn(targets, n, &e);
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    fn targets_label(&self, targets: &[usize]) -> String {
        match targets {
            [n] => self.state.inboxes[*n].address.clone(),
            _ => format!("{} inboxes", targets.len()),
        }
    }

    /// In multi-inbox mode: stop polling an inbox that is gone for good,
    /// and only warn about temporary failures.
    fn skip_or_warn(&self, targets: &mut Vec<usize>, n: usize, e: &Error) {
        let address = &self.state.inboxes[n].address;
        if e.is_provider_failure() {
            self.warn(&format!("{e}; will retry {address}"));
        } else {
            self.note(&format!("skipping {address}: {e}"));
            targets.retain(|&t| t != n);
        }
    }

    fn otp(&mut self, a: OtpArgs) -> Result<()> {
        let re = a.regex.as_deref().map(compile_plain).transpose()?;
        let (inbox, msg, body, code) = if a.wait {
            let (n, msg, body, code) = self.wait_for(&a.filter, |m, body| {
                extract::otp(&m.subject, body, re.as_ref())
            })?;
            (self.state.inboxes[n].address.clone(), msg, body, Some(code))
        } else {
            let msg = self.fetch(&a.msg)?;
            let body = message_body(&msg);
            let code = extract::otp(&msg.subject, &body, re.as_ref());
            (self.current()?.address.clone(), msg, body, code)
        };
        match code {
            Some(code) => {
                self.print_code(&code, &msg, &body, re.is_some(), &inbox);
                Ok(())
            }
            None => Err(Error::NotFound(format!(
                "no verification code found in message {} ({:?}); try --regex",
                msg.id,
                clean_line(&msg.subject)
            ))),
        }
    }

    fn links(&mut self, a: LinksArgs) -> Result<()> {
        let filter = a.r#match.as_deref().map(compile).transpose()?;
        let msg = self.fetch(&a.msg)?;
        let mut links = message_links(&msg);
        if let Some(re) = &filter {
            links.retain(|l| re.is_match(l));
        }
        if links.is_empty() {
            return Err(Error::NotFound(format!(
                "no links found in message {}",
                msg.id
            )));
        }
        if a.first || a.open {
            links.truncate(1);
        }
        if a.open {
            system::open(&links[0], self.browser().as_deref())?;
        }
        if self.global.json {
            self.json_out(&json!({ "links": links, "message_id": msg.id }));
        } else {
            for l in &links {
                println!("{}", clean_line(l));
            }
        }
        Ok(())
    }

    fn watch(&mut self, a: WatchArgs) -> Result<()> {
        let interval = self.config.poll_interval(a.interval);
        let mut targets = self.targets(a.all_inboxes)?;
        let multi = a.all_inboxes;
        let mut seen: HashSet<(usize, String)> = HashSet::new();
        let mut first = true;
        let mut failing: HashSet<usize> = HashSet::new();
        let mut poller = Poller::new(Duration::from_secs(interval), self.config.push());
        let mut rotate_at = a.rotate.map(|every| Instant::now() + every);
        self.note(&format!(
            "watching {}, Ctrl+C to stop",
            self.targets_label(&targets)
        ));
        loop {
            if let (Some(at), Some(every)) = (rotate_at, a.rotate) {
                if Instant::now() >= at {
                    rotate_at = Some(match self.watch_rotate(&a, &mut poller) {
                        Ok(n) => {
                            targets = vec![n];
                            // Indexes moved; the new inbox has nothing seen.
                            seen.clear();
                            failing.clear();
                            Instant::now() + every
                        }
                        Err(e) => {
                            self.warn(&format!("could not rotate ({e}); retrying in a minute"));
                            Instant::now() + Duration::from_secs(60)
                        }
                    });
                }
            }
            for n in targets.clone() {
                if !poller.is_due(&self.state.inboxes[n].address) {
                    continue;
                }
                let Some(list) = self.poll(n, &mut targets, &mut failing, multi, &mut poller)?
                else {
                    continue;
                };
                let address = clean_line(&self.state.inboxes[n].address);
                for s in list.iter().rev() {
                    if !seen.insert((n, s.id.clone())) {
                        continue;
                    }
                    let line = if multi {
                        format!("{address}  {}", summary_line(s))
                    } else {
                        summary_line(s)
                    };
                    if first && !self.global.json {
                        // Show what's already there, without notifying.
                        println!("{}", self.dim(&line));
                        continue;
                    }
                    if self.global.json {
                        let mut v = serde_json::to_value(s).expect("serializable");
                        v["inbox"] = json!(address);
                        println!("{v}");
                    } else {
                        println!("{line}");
                    }
                    if a.notify && !first {
                        let title = if multi {
                            format!("{} → {address}", clean_line(&s.from))
                        } else {
                            clean_line(&s.from)
                        };
                        system::notify(&title, &clean_line(&s.subject));
                    }
                }
            }
            let _ = std::io::stdout().flush();
            first = false;
            if targets.is_empty() {
                return Err(Error::NotFound("all inboxes have expired".into()));
            }
            poller.sleep(rotate_at);
        }
    }

    /// `watch --rotate`: replaces the watched inbox with a fresh one, makes
    /// it active and announces it. Returns the new inbox's index.
    fn watch_rotate(&mut self, a: &WatchArgs, poller: &mut Poller) -> Result<usize> {
        let old = self.current()?.address.clone();
        let inbox = self.replace_current(a.keep)?;
        let added = inbox.clone();
        self.state = self.store.update(|s| s.add(added))?.0;
        // `-i` named the old inbox; from now on follow the active one.
        self.global.inbox = None;
        poller.forget(&old);
        let address = clean_line(&inbox.address);
        if a.copy {
            if let Err(e) = system::copy(&inbox.address, self.clipboard_cmd().as_deref()) {
                self.warn(&format!("could not copy the new address: {e:#}"));
            }
        }
        if self.global.json {
            println!(
                "{}",
                json!({ "rotated": { "from": old, "to": inbox.address } })
            );
        } else {
            println!("{} {}", self.green("new address:"), self.bold(&address));
        }
        if a.notify {
            system::notify("tmpmail: new address", &address);
        }
        self.state
            .position(&inbox.address)
            .ok_or_else(|| Error::NotFound(format!("{address} vanished from the state file")))
    }

    // ----- attachments -----

    fn attachments(&mut self, a: MessageRef) -> Result<()> {
        let msg = self.fetch(&a)?;
        if self.global.json {
            self.json_out(&json!({ "message_id": msg.id, "attachments": msg.attachments }));
            return Ok(());
        }
        if msg.attachments.is_empty() {
            println!("No attachments.");
        }
        let rows: Vec<[String; 4]> = msg
            .attachments
            .iter()
            .map(|att| {
                [
                    clean_line(&att.id),
                    clean_line(&att.filename),
                    clean_line(&att.content_type),
                    att.size.map(human_size).unwrap_or_default(),
                ]
            })
            .collect();
        let [iw, fw, tw, sw] = column_widths(&rows);
        for [id, file, kind, size] in &rows {
            println!(
                "{}  {}  {}  {size:>sw$}",
                self.dim(&pad(id, iw)),
                pad(file, fw),
                self.dim(&pad(kind, tw)),
            );
        }
        Ok(())
    }

    fn save(&mut self, a: SaveArgs) -> Result<()> {
        let (wanted, out_dir) = (&a.attachments, &a.output);
        std::fs::create_dir_all(out_dir).map_err(anyhow::Error::from)?;
        let saved = self.with_message(Some(&a.id), |p, inbox, id| {
            let msg = p.get(inbox, id)?;
            let selected: Vec<_> = msg
                .attachments
                .iter()
                .filter(|att| {
                    wanted.is_empty() || wanted.iter().any(|w| *w == att.id || *w == att.filename)
                })
                .collect();
            if selected.is_empty() {
                return Err(Error::NotFound(format!(
                    "no matching attachments in message {id}"
                )));
            }
            let mut saved = Vec::new();
            for (n, att) in selected.iter().enumerate() {
                let data = p.attachment(inbox, &msg, att)?;
                let name = safe_filename(&att.filename, &format!("attachment-{}", n + 1));
                let path = unique_path(&out_dir.join(&name));
                std::fs::write(&path, &data).map_err(anyhow::Error::from)?;
                saved.push(path);
            }
            Ok(saved)
        })?;
        if self.global.json {
            self.json_out(&json!({ "saved": saved }));
        } else {
            for p in saved {
                println!("{}", p.display());
            }
        }
        Ok(())
    }

    fn delete(&mut self, a: DeleteArgs) -> Result<()> {
        let deleted = if a.all {
            self.with_inbox(|p, inbox| {
                let list = p.list(inbox)?;
                for s in &list {
                    p.delete_message(inbox, &s.id)?;
                }
                Ok(list.len())
            })?
        } else {
            self.with_message(a.id.as_deref(), |p, inbox, id| {
                p.delete_message(inbox, id).map(|()| 1)
            })?
        };
        if self.global.json {
            self.json_out(&json!({ "deleted": deleted }));
        } else {
            self.note(&format!("deleted {deleted} message(s)"));
        }
        Ok(())
    }

    // ----- inbox management -----

    fn whoami(&mut self) -> Result<()> {
        let inbox = self.current()?;
        if self.global.json {
            self.json_out(&self.view(inbox));
        } else {
            println!("{}", clean_line(&inbox.address));
            if inbox.is_expired() {
                self.warn("this inbox has expired; create a new one with `tmpmail new`");
            }
        }
        Ok(())
    }

    fn copy(&mut self) -> Result<()> {
        let address = self.current()?.address.clone();
        system::copy(&address, self.clipboard_cmd().as_deref())?;
        if self.global.json {
            self.json_out(&json!({ "copied": address }));
        } else {
            self.note(&format!("copied {address}"));
        }
        Ok(())
    }

    fn qr(&mut self) -> Result<()> {
        let address = self.current()?.address.clone();
        let code = qrcode::QrCode::new(address.as_bytes()).map_err(|e| anyhow::anyhow!("{e}"))?;
        let art = code
            .render::<qrcode::render::unicode::Dense1x2>()
            .dark_color(qrcode::render::unicode::Dense1x2::Light)
            .light_color(qrcode::render::unicode::Dense1x2::Dark)
            .quiet_zone(true)
            .build();
        println!("{art}\n{}", clean_line(&address));
        Ok(())
    }

    fn domains(&mut self, a: ProviderArg) -> Result<()> {
        let id = a
            .provider
            .clone()
            .unwrap_or_else(|| self.config.provider_order()[0].clone());
        let domains = provider::get(&id)?.domains()?;
        if self.global.json {
            self.json_out(&json!({ "provider": id, "domains": domains }));
        } else {
            for d in domains {
                println!("{d}");
            }
        }
        Ok(())
    }

    fn inboxes(&mut self, a: InboxesArgs) -> Result<()> {
        if a.prune {
            let (state, pruned) = self.store.update(State::prune_expired)?;
            self.state = state;
            self.note(&format!("forgot {pruned} expired inbox(es)"));
        }
        if self.global.json {
            let views: Vec<_> = self.state.inboxes.iter().map(|i| self.view(i)).collect();
            self.json_out(&views);
            return Ok(());
        }
        if self.state.inboxes.is_empty() {
            println!("No inboxes. Create one with `tmpmail new`.");
        }
        let rows: Vec<[String; 3]> = self
            .state
            .inboxes
            .iter()
            .map(|i| {
                [
                    clean_line(&i.address),
                    i.label
                        .as_deref()
                        .map(|l| format!("[{}]", clean_line(l)))
                        .unwrap_or_default(),
                    i.provider.clone(),
                ]
            })
            .collect();
        let [aw, lw, pw] = column_widths(&rows);
        for (i, [address, label, provider]) in self.state.inboxes.iter().zip(&rows) {
            let v = self.view(i);
            let mark = if v.active {
                self.green("*")
            } else {
                " ".into()
            };
            let status = match i.expires_at {
                _ if v.expired => self.red("expired"),
                Some(t) if t - Utc::now() < chrono::Duration::minutes(10) => {
                    self.yellow(&format!("expires {}", relative(t)))
                }
                Some(t) => self.dim(&format!("expires {}", relative(t))),
                None => String::new(),
            };
            let line = format!(
                "{mark} {}  {}  {}  {status}",
                self.bold(&pad(address, aw)),
                self.cyan(&pad(label, lw)),
                self.dim(&pad(provider, pw)),
            );
            println!("{}", line.trim_end());
        }
        Ok(())
    }

    fn use_inbox(&mut self, address: &str) -> Result<()> {
        let n = self.state.resolve(Some(address))?;
        let address = self.state.inboxes[n].address.clone();
        let active = address.clone();
        self.state = self.store.update(|s| s.active = Some(active))?.0;
        if self.global.json {
            if let Some(n) = self.state.position(&address) {
                self.json_out(&self.view(&self.state.inboxes[n]));
            }
        } else {
            println!("{}", clean_line(&address));
        }
        Ok(())
    }

    fn login(&mut self, a: LoginArgs) -> Result<()> {
        let id = a.provider.clone().unwrap_or_else(|| {
            provider::all()
                .find(|id| provider::get(id).is_ok_and(|p| p.owns_address(&a.address)))
                .unwrap_or("mailtm")
                .to_string()
        });
        let p = provider::get(&id)?;
        let password = match (&a.password, p.caps().private) {
            (Some(pw), _) => pw.clone(),
            (None, false) => String::new(),
            (None, true) => {
                return Err(Error::Usage(
                    "--password (or TMPMAIL_PASSWORD) is required".into(),
                ));
            }
        };
        let mut inbox = checked_address(p.as_ref(), p.login(&a.address, &password)?)?;
        inbox.label = a.label;
        self.activate(&inbox, false)
    }

    fn drop_inbox(&mut self, a: DropArgs) -> Result<()> {
        let n = self
            .state
            .resolve(a.address.as_deref().or(self.global.inbox.as_deref()))?;
        let mut inbox = self.state.inboxes[n].clone();
        if !a.local {
            let p = provider::get(&inbox.provider)?;
            if let Err(e) = p.delete_inbox(&mut inbox) {
                self.warn(&format!(
                    "could not delete it at the provider ({e}); forgetting it locally"
                ));
            }
        }
        self.forget(&inbox.address)?;
        if self.global.json {
            self.json_out(&json!({ "dropped": inbox.address, "active": self.state.active }));
        } else {
            self.note(&format!("dropped {}", inbox.address));
        }
        Ok(())
    }

    fn rotate(&mut self, a: RotateArgs) -> Result<()> {
        let inbox = self.replace_current(a.keep)?;
        self.activate(&inbox, a.copy)
    }

    /// Creates a fresh inbox to replace the selected one, with the same
    /// provider if it works, and drops the old one unless `keep`. The caller
    /// stores the new inbox.
    fn replace_current(&mut self, keep: bool) -> Result<Inbox> {
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
    fn forget(&mut self, address: &str) -> Result<()> {
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

    fn providers(&mut self, a: ProvidersArgs) -> Result<()> {
        let order = self.config.provider_order();
        // Health checks are independent network calls: run them side by side.
        let mut health: Vec<Option<Health>> = if a.check {
            std::thread::scope(|s| {
                let checks: Vec<_> = provider::all()
                    .map(|id| s.spawn(move || check_health(id)))
                    .collect();
                checks
                    .into_iter()
                    .map(|c| Some(c.join().expect("health check panicked")))
                    .collect()
            })
        } else {
            provider::all().map(|_| None).collect()
        };
        let mut rows = Vec::new();
        for (id, status) in provider::all().zip(health.drain(..)) {
            let p = provider::get(id)?;
            rows.push(ProviderRow {
                id,
                name: p.name(),
                website: p.website(),
                in_fallback_order: order.iter().position(|o| o == id).map(|n| n + 1),
                capabilities: p.caps(),
                status,
            });
        }
        if self.global.json {
            self.json_out(&rows);
            return Ok(());
        }
        for r in &rows {
            let caps = &r.capabilities;
            let order = match r.in_fallback_order {
                Some(n) => format!("#{n}"),
                None => "--".into(),
            };
            let flags: Vec<&str> = [
                (!caps.private, "PUBLIC"),
                (caps.custom_user, "custom-user"),
                (caps.attachments, "attachments"),
                (caps.delete_message, "delete"),
            ]
            .into_iter()
            .filter_map(|(on, flag)| on.then_some(flag))
            .collect();
            let status = match &r.status {
                None => String::new(),
                Some(Health { ms: Some(ms), .. }) => {
                    format!("  {}", self.green(&format!("ok {ms}ms")))
                }
                Some(h) => format!(
                    "  {} {}",
                    self.red("DOWN:"),
                    clean_line(h.error.as_deref().unwrap_or_default())
                ),
            };
            let about = if flags.is_empty() {
                caps.lifetime.to_string()
            } else {
                format!("{} | {}", flags.join(","), caps.lifetime)
            };
            println!(
                "{order:>3} {:<13} {:<15} {}{}",
                r.id,
                r.name,
                self.dim(&about),
                status
            );
        }
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

fn check_health(id: &str) -> Health {
    let start = Instant::now();
    match provider::get(id).and_then(|p| p.health()) {
        Ok(()) => Health {
            ok: true,
            ms: Some(start.elapsed().as_millis() as u64),
            error: None,
        },
        Err(e) => Health {
            ok: false,
            ms: None,
            error: Some(e.to_string()),
        },
    }
}

/// Rejects an address a provider returned that couldn't be a real one, such
/// as one carrying terminal escapes: it is printed and stored as is.
fn checked_address(p: &dyn Provider, inbox: Inbox) -> Result<Inbox> {
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

fn compile(pattern: &str) -> Result<Regex> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .build()
        .map_err(|e| Error::Usage(format!("invalid regex {pattern:?}: {e}")))
}

fn compile_plain(pattern: &str) -> Result<Regex> {
    Regex::new(pattern).map_err(|e| Error::Usage(format!("invalid regex {pattern:?}: {e}")))
}

fn message_body(m: &Message) -> String {
    body_text(m.text.as_deref(), m.html.as_deref(), 200)
}

fn message_links(m: &Message) -> Vec<String> {
    extract::links(
        m.text.as_deref().unwrap_or_default(),
        m.html.as_deref().unwrap_or_default(),
    )
}

fn summary_line(s: &Summary) -> String {
    let when = s.date.map(relative).unwrap_or_default();
    format!(
        "{when:>9}  {}  {}",
        clean_line(&s.from),
        clean_line(&s.subject)
    )
}

fn term_width() -> usize {
    system::term_size().map_or(100, |(cols, _)| cols)
}

/// Widest cell of each column, in characters.
fn column_widths<const N: usize>(rows: &[[String; N]]) -> [usize; N] {
    let mut widths = [0; N];
    for row in rows {
        for (w, cell) in widths.iter_mut().zip(row) {
            *w = (*w).max(cell.chars().count());
        }
    }
    widths
}

fn pad(s: &str, width: usize) -> String {
    format!("{s:<width$}")
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn relative(t: DateTime<Utc>) -> String {
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

fn human_size(n: u64) -> String {
    match n {
        0..1024 => format!("{n} B"),
        1024..1_048_576 => format!("{:.1} KB", n as f64 / 1024.0),
        _ => format!("{:.1} MB", n as f64 / 1_048_576.0),
    }
}

/// `file.pdf` -> `file (1).pdf` if it already exists.
fn unique_path(path: &std::path::Path) -> std::path::PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = path.extension().and_then(|s| s.to_str());
    for n in 1.. {
        let name = match ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = path.with_file_name(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn unique_paths_do_not_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        assert_eq!(unique_path(&p), p);
        std::fs::write(&p, "x").unwrap();
        assert_eq!(unique_path(&p), dir.path().join("a (1).txt"));
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

    #[test]
    fn truncation_and_sizes() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
        assert_eq!(human_size(2048), "2.0 KB");
    }

    #[test]
    fn columns_fit_the_widest_cell() {
        let rows = [
            ["a@x.com".to_string(), String::new()],
            ["long@example.org".to_string(), "[github]".to_string()],
        ];
        assert_eq!(column_widths(&rows), [16, 8]);
        assert_eq!(pad("ab", 4), "ab  ");
    }
}
