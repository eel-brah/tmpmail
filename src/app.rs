use std::collections::HashSet;
use std::io::{IsTerminal, Write};
use std::thread::sleep;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use regex::{Regex, RegexBuilder};
use serde::Serialize;
use serde_json::{Value, json};

use crate::cli::*;
use crate::config::Config;
use crate::error::{Error, Result};
use crate::extract;
use crate::provider::{self, CreateOpts, Inbox, Message, Provider, Summary};
use crate::render::{body_text, clean, clean_line, safe_filename, safe_html_document};
use crate::store::{State, Store};
use crate::system;

pub struct App {
    pub global: Global,
    pub config: Config,
    pub store: Store,
    pub state: State,
    color: bool,
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
            Command::Completions { .. } => unreachable!("handled in main"),
        }
    }

    // ---------- output helpers ----------

    fn bold(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[1m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    fn dim(&self, s: &str) -> String {
        if self.color {
            format!("\x1b[2m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    fn json_out(&self, v: &impl Serialize) {
        println!("{}", serde_json::to_string_pretty(v).expect("serializable"));
    }

    fn note(&self, msg: &str) {
        if !self.global.quiet && !self.global.json {
            eprintln!("{}", self.dim(msg));
        }
    }

    fn warn(&self, msg: &str) {
        eprintln!("tmpmail: {msg}");
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
        let p = provider::get(&inbox.provider)?;
        let result = f(p.as_ref(), &mut inbox);
        if inbox != self.state.inboxes[n] {
            self.state.inboxes[n] = inbox;
            self.store.save(&self.state)?;
        }
        result
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

    fn fetch(&mut self, msg: &MessageRef) -> Result<Message> {
        let id = msg.id.clone();
        self.with_inbox(|p, inbox| {
            let id = Self::resolve_id(p, inbox, id.as_deref())?;
            p.get(inbox, &id)
        })
    }

    fn save_state(&self) -> Result<()> {
        self.store.save(&self.state)
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
        let inbox = self.create_with_fallback(a.provider.as_deref(), &opts)?;
        let mut inbox = inbox;
        inbox.label = a.label;
        let p = provider::get(&inbox.provider)?;
        self.state.add(inbox.clone());
        self.save_state()?;

        if a.copy {
            system::copy(&inbox.address, self.clipboard_cmd().as_deref())?;
        }
        if self.global.json {
            self.json_out(&self.view(&inbox));
        } else {
            println!("{}", inbox.address);
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

    fn create_with_fallback(&self, explicit: Option<&str>, opts: &CreateOpts) -> Result<Inbox> {
        if let Some(id) = explicit {
            return provider::get(id)?.create(opts);
        }
        let order = self.config.provider_order();
        let mut failures = Vec::new();
        for id in &order {
            let p = provider::get(id)?;
            let caps = p.caps();
            if opts.user.is_some() && !caps.custom_user
                || opts.domain.is_some() && !caps.choose_domain
                || opts.password.is_some() && !caps.login
            {
                continue;
            }
            match p.create(opts) {
                Ok(inbox) => {
                    if !failures.is_empty() {
                        self.note(&format!("using {} instead", p.name()));
                    }
                    return Ok(inbox);
                }
                Err(e) if e.is_provider_failure() => {
                    self.warn(&format!("{e}, trying the next provider"));
                    failures.push(e.to_string());
                }
                // A requested domain may belong to a later provider.
                Err(Error::Usage(msg)) if opts.domain.is_some() => failures.push(msg),
                Err(e) => return Err(e),
            }
        }
        if failures.is_empty() {
            return Err(Error::Usage(
                "no configured provider supports these options (see `tmpmail providers`)".into(),
            ));
        }
        Err(Error::Unreachable {
            provider: "all providers".into(),
            detail: failures.join("; "),
        })
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
            let result = self.with_inbox_at(n, |p, inbox| Self::sorted_list(p, inbox));
            match result {
                Ok(list) => sections.push((address, Ok(list))),
                // With -a, one dead inbox must not hide the others.
                Err(e) if a.all_inboxes => sections.push((address, Err(e))),
                Err(e) => return Err(e),
            }
        }

        if self.global.json {
            if !a.all_inboxes {
                let (address, list) = sections.remove(0);
                self.json_out(&json!({ "address": address, "messages": list? }));
            } else {
                let all: Vec<Value> = sections
                    .iter()
                    .map(|(address, r)| match r {
                        Ok(list) => json!({ "address": address, "messages": list }),
                        Err(e) => json!({
                            "address": address,
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
        for (k, (address, result)) in sections.into_iter().enumerate() {
            if k > 0 {
                println!();
            }
            if !self.global.quiet || total > 1 {
                println!("{}", self.bold(&format!("[ Inbox for {address} ]")));
            }
            match result {
                Ok(list) if list.is_empty() => {
                    if !self.global.quiet {
                        println!("No messages yet. `tmpmail wait` blocks until one arrives.");
                    }
                }
                Ok(list) => self.print_list(&list),
                Err(e) => println!("{}", self.dim(&format!("  unavailable: {e}"))),
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
            let marker = if s.seen { ' ' } else { '*' };
            let att = if s.has_attachments { " [att]" } else { "" };
            let when = s.date.map(relative).unwrap_or_default();
            let from = truncate(&clean_line(&s.from), 28);
            let room = width.saturating_sub(4 + 10 + 30 + att.len()).max(20);
            let subject = truncate(&clean_line(&s.subject), room);
            println!(
                "{:>3}{marker} {}  {from:<28}  {}{att}",
                n + 1,
                self.dim(&format!("{when:>8}")),
                self.bold(&subject),
            );
        }
    }

    fn read(&mut self, a: ReadArgs) -> Result<()> {
        if a.raw {
            let id = a.msg.id.clone();
            let raw = self.with_inbox(|p, inbox| {
                let id = Self::resolve_id(p, inbox, id.as_deref())?;
                p.raw(inbox, &id)
            })?;
            std::io::stdout()
                .write_all(&raw)
                .map_err(anyhow::Error::from)?;
            return Ok(());
        }
        let msg = self.fetch(&a.msg)?;
        if a.html {
            return self.open_html(&msg, a.images);
        }
        self.print_message(&msg);
        Ok(())
    }

    fn print_message(&self, msg: &Message) {
        if self.global.json {
            self.json_out(msg);
            return;
        }
        let width = term_width().min(100);
        let header =
            |k: &str, v: &str| println!("{} {}", self.bold(&format!("{k:<8}")), clean_line(v));
        header("From:", &msg.from);
        header("To:", &msg.to.join(", "));
        header("Subject:", &msg.subject);
        if let Some(d) = msg.date {
            header(
                "Date:",
                &d.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string(),
            );
        }
        if !msg.attachments.is_empty() {
            let names: Vec<String> = msg.attachments.iter().map(|a| a.filename.clone()).collect();
            header(
                "Attach:",
                &format!("{} (tmpmail save {})", names.join(", "), msg.id),
            );
        }
        println!();
        let body = body_text(msg.text.as_deref(), msg.html.as_deref(), width);
        println!("{}", clean(body.trim_end()));
        if msg.html.is_some() && !self.global.quiet {
            self.note(&format!("\n(HTML version: tmpmail read {} --html)", msg.id));
        }
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
        let want_otp = a.otp;
        let want_link = a.link;
        let (n, msg) = self.wait_for(&a.filter, |m| {
            let (subject, body) = (&m.subject, message_body(m));
            if want_otp {
                extract::otp(subject, &body, otp_re.as_ref()).is_some()
            } else if want_link {
                !extract::links(
                    m.text.as_deref().unwrap_or_default(),
                    m.html.as_deref().unwrap_or_default(),
                )
                .is_empty()
            } else {
                true
            }
        })?;

        if want_otp {
            let code = extract::otp(&msg.subject, &message_body(&msg), otp_re.as_ref())
                .unwrap_or_default();
            self.print_code(&code, &msg, n);
        } else if want_link {
            let link = extract::links(
                msg.text.as_deref().unwrap_or_default(),
                msg.html.as_deref().unwrap_or_default(),
            )
            .into_iter()
            .next()
            .unwrap_or_default();
            if self.global.json {
                self.json_out(&json!({
                    "link": link,
                    "message_id": msg.id,
                    "inbox": self.state.inboxes[n].address,
                }));
            } else {
                println!("{}", clean_line(&link));
            }
        } else if self.global.json {
            let mut v = serde_json::to_value(&msg).expect("serializable");
            v["inbox"] = json!(self.state.inboxes[n].address);
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

    fn print_code(&self, code: &str, msg: &Message, n: usize) {
        if self.global.json {
            self.json_out(&json!({
                "code": code,
                "message_id": msg.id,
                "from": msg.from,
                "subject": msg.subject,
                "inbox": self.state.inboxes[n].address,
            }));
        } else {
            println!("{}", clean_line(code));
        }
    }

    /// Polls the target inbox(es) until a message passes the filter and
    /// `accept`, or times out. Returns the inbox index and the message.
    fn wait_for(
        &mut self,
        f: &WaitFilter,
        accept: impl Fn(&Message) -> bool,
    ) -> Result<(usize, Message)> {
        let from = f.from.as_deref().map(compile).transpose()?;
        let subject = f.subject.as_deref().map(compile).transpose()?;
        let body = f.body.as_deref().map(compile).transpose()?;
        let timeout = f.timeout.unwrap_or_else(|| self.config.timeout());
        let interval = f
            .interval
            .unwrap_or_else(|| self.config.poll_interval())
            .max(2);
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
        let what = if targets.len() == 1 {
            self.state.inboxes[targets[0]].address.clone()
        } else {
            format!("{} inboxes", targets.len())
        };
        self.note(&format!(
            "waiting for mail to {what} (timeout {timeout}s)..."
        ));

        let mut warned: HashSet<usize> = HashSet::new();
        loop {
            for n in targets.clone() {
                let list = match self.with_inbox_at(n, |p, inbox| Self::sorted_list(p, inbox)) {
                    Ok(list) => list,
                    Err(e) if e.is_provider_failure() => {
                        if warned.insert(n) {
                            self.warn(&format!("{e}; retrying until the timeout"));
                        }
                        continue;
                    }
                    Err(e) if all => {
                        self.skip_or_warn(&mut targets, n, &e);
                        continue;
                    }
                    Err(e) => return Err(e),
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
                    if body
                        .as_ref()
                        .is_some_and(|re| !re.is_match(&message_body(&msg)))
                    {
                        continue;
                    }
                    if accept(&msg) {
                        if all {
                            self.note(&format!("received by {}", self.state.inboxes[n].address));
                        }
                        return Ok((n, msg));
                    }
                }
            }
            if targets.is_empty() {
                return Err(Error::NotFound("all inboxes have expired".into()));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(Error::Timeout(timeout));
            }
            let jitter = Duration::from_millis(rand::random_range(0..1000));
            sleep((Duration::from_secs(interval) + jitter).min(deadline - now));
            if Instant::now() >= deadline {
                return Err(Error::Timeout(timeout));
            }
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
        let (inbox, msg) = if a.wait {
            let (n, msg) = self.wait_for(&a.filter, |m| {
                extract::otp(&m.subject, &message_body(m), re.as_ref()).is_some()
            })?;
            (self.state.inboxes[n].address.clone(), msg)
        } else {
            let msg = self.fetch(&a.msg)?;
            (self.current()?.address.clone(), msg)
        };
        let body = message_body(&msg);
        match extract::otp(&msg.subject, &body, re.as_ref()) {
            Some(code) => {
                if self.global.json {
                    self.json_out(&json!({
                        "code": code,
                        "candidates": extract::otp_candidates(&msg.subject, &body),
                        "inbox": inbox,
                        "message_id": msg.id,
                        "from": msg.from,
                        "subject": msg.subject,
                    }));
                } else {
                    println!("{}", clean_line(&code));
                }
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
        let mut links = extract::links(
            msg.text.as_deref().unwrap_or_default(),
            msg.html.as_deref().unwrap_or_default(),
        );
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
        let interval = a
            .interval
            .unwrap_or_else(|| self.config.poll_interval())
            .max(2);
        let mut targets = self.targets(a.all_inboxes)?;
        let multi = a.all_inboxes;
        let mut seen: HashSet<(usize, String)> = HashSet::new();
        let mut first = true;
        let mut failing: HashSet<usize> = HashSet::new();
        let what = if multi {
            format!("{} inboxes", targets.len())
        } else {
            self.state.inboxes[targets[0]].address.clone()
        };
        self.note(&format!("watching {what}, Ctrl+C to stop"));
        loop {
            for n in targets.clone() {
                let address = self.state.inboxes[n].address.clone();
                let list = match self.with_inbox_at(n, |p, inbox| Self::sorted_list(p, inbox)) {
                    Ok(list) => list,
                    Err(e) if e.is_provider_failure() => {
                        if failing.insert(n) {
                            self.warn(&format!("{e}; will keep trying"));
                        }
                        continue;
                    }
                    Err(e) if multi => {
                        self.skip_or_warn(&mut targets, n, &e);
                        continue;
                    }
                    Err(e) => return Err(e),
                };
                if failing.remove(&n) {
                    self.note(&format!("{address} reachable again"));
                }
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
            sleep(Duration::from_secs(interval));
        }
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
        for att in &msg.attachments {
            let size = att.size.map(human_size).unwrap_or_default();
            println!(
                "{}  {}  {}  {}",
                att.id,
                clean_line(&att.filename),
                self.dim(&att.content_type),
                size
            );
        }
        Ok(())
    }

    fn save(&mut self, a: SaveArgs) -> Result<()> {
        let id = a.id.clone();
        let wanted = a.attachments.clone();
        let out_dir = a.output.clone();
        std::fs::create_dir_all(&out_dir).map_err(anyhow::Error::from)?;
        let saved = self.with_inbox(|p, inbox| {
            let id = Self::resolve_id(p, inbox, Some(&id))?;
            let msg = p.get(inbox, &id)?;
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
        let id = a.id.clone();
        let deleted = self.with_inbox(|p, inbox| {
            if a.all {
                let list = p.list(inbox)?;
                for s in &list {
                    p.delete_message(inbox, &s.id)?;
                }
                Ok(list.len())
            } else {
                let id = Self::resolve_id(p, inbox, id.as_deref())?;
                p.delete_message(inbox, &id)?;
                Ok(1)
            }
        })?;
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
            println!("{}", inbox.address);
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
        println!("{art}\n{address}");
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
            let before = self.state.inboxes.len();
            self.state.inboxes.retain(|i| !i.is_expired());
            if self
                .state
                .active
                .as_ref()
                .is_some_and(|act| !self.state.inboxes.iter().any(|i| &i.address == act))
            {
                self.state.active = self.state.inboxes.last().map(|i| i.address.clone());
            }
            self.save_state()?;
            self.note(&format!(
                "forgot {} expired inbox(es)",
                before - self.state.inboxes.len()
            ));
        }
        if self.global.json {
            let views: Vec<_> = self.state.inboxes.iter().map(|i| self.view(i)).collect();
            self.json_out(&views);
            return Ok(());
        }
        if self.state.inboxes.is_empty() {
            println!("No inboxes. Create one with `tmpmail new`.");
        }
        for i in &self.state.inboxes {
            let v = self.view(i);
            let mark = if v.active { "*" } else { " " };
            let label = i
                .label
                .as_deref()
                .map(|l| format!(" [{l}]"))
                .unwrap_or_default();
            let status = match i.expires_at {
                _ if v.expired => "expired".to_string(),
                Some(t) => format!("expires {}", relative(t)),
                None => String::new(),
            };
            println!(
                "{mark} {}{label}  {}  {}",
                self.bold(&i.address),
                self.dim(&i.provider),
                self.dim(&status)
            );
        }
        Ok(())
    }

    fn use_inbox(&mut self, address: &str) -> Result<()> {
        let n = self
            .state
            .find(address)
            .ok_or_else(|| Error::NotFound(format!("no stored inbox matches {address:?}")))?;
        let address = self.state.inboxes[n].address.clone();
        self.state.active = Some(address.clone());
        self.save_state()?;
        if self.global.json {
            self.json_out(&self.view(&self.state.inboxes[n]));
        } else {
            println!("{address}");
        }
        Ok(())
    }

    fn login(&mut self, a: LoginArgs) -> Result<()> {
        let id = a.provider.clone().unwrap_or_else(|| {
            if a.address.ends_with("@maildrop.cc") {
                "maildrop".into()
            } else {
                "mailtm".into()
            }
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
        let mut inbox = p.login(&a.address, &password)?;
        inbox.label = a.label;
        self.state.add(inbox.clone());
        self.save_state()?;
        if self.global.json {
            self.json_out(&self.view(&inbox));
        } else {
            println!("{}", inbox.address);
        }
        Ok(())
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
        let removed = self.state.remove(n);
        self.save_state()?;
        if self.global.json {
            self.json_out(&json!({ "dropped": removed.address, "active": self.state.active }));
        } else {
            self.note(&format!("dropped {}", removed.address));
        }
        Ok(())
    }

    fn rotate(&mut self, a: RotateArgs) -> Result<()> {
        let old = self.current()?.clone();
        // Prefer the same provider, fall back like `new` does.
        let opts = CreateOpts::default();
        let inbox = match provider::get(&old.provider)?.create(&opts) {
            Ok(i) => i,
            Err(e) if e.is_provider_failure() => {
                self.warn(&format!("{e}, trying other providers"));
                self.create_with_fallback(None, &opts)?
            }
            Err(e) => return Err(e),
        };
        let mut inbox = inbox;
        inbox.label = old.label.clone();
        if !a.keep {
            if let Some(n) = self.state.find(&old.address) {
                let mut gone = self.state.remove(n);
                let _ = provider::get(&gone.provider).map(|p| p.delete_inbox(&mut gone));
            }
        }
        self.state.add(inbox.clone());
        self.save_state()?;
        if a.copy {
            system::copy(&inbox.address, self.clipboard_cmd().as_deref())?;
        }
        if self.global.json {
            self.json_out(&self.view(&inbox));
        } else {
            println!("{}", inbox.address);
        }
        Ok(())
    }

    fn providers(&mut self, a: ProvidersArgs) -> Result<()> {
        let order = self.config.provider_order();
        let mut rows = Vec::new();
        for id in provider::ALL {
            let p = provider::get(id)?;
            let status = if a.check {
                let start = Instant::now();
                Some(match p.health() {
                    Ok(()) => json!({ "ok": true, "ms": start.elapsed().as_millis() as u64 }),
                    Err(e) => json!({ "ok": false, "error": e.to_string() }),
                })
            } else {
                None
            };
            rows.push(json!({
                "id": id,
                "name": p.name(),
                "website": p.website(),
                "in_fallback_order": order.iter().position(|o| o == id).map(|n| n + 1),
                "capabilities": p.caps(),
                "status": status,
            }));
        }
        if self.global.json {
            self.json_out(&rows);
            return Ok(());
        }
        for r in &rows {
            let caps = &r["capabilities"];
            let order = match r["in_fallback_order"].as_u64() {
                Some(n) => format!("#{n}"),
                None => "--".into(),
            };
            let mut flags = Vec::new();
            if caps["private"] == false {
                flags.push("PUBLIC");
            }
            if caps["custom_user"] == true {
                flags.push("custom-user");
            }
            if caps["attachments"] == true {
                flags.push("attachments");
            }
            if caps["delete_message"] == true {
                flags.push("delete");
            }
            let status = match &r["status"] {
                Value::Null => String::new(),
                s if s["ok"] == true => format!("  ok {}ms", s["ms"]),
                s => format!("  DOWN: {}", s["error"].as_str().unwrap_or_default()),
            };
            println!(
                "{order:>3} {:<13} {:<15} {}{}",
                r["id"].as_str().unwrap_or_default(),
                r["name"].as_str().unwrap_or_default(),
                self.dim(&format!(
                    "{} | {}",
                    flags.join(","),
                    caps["lifetime"].as_str().unwrap_or_default()
                )),
                status
            );
        }
        Ok(())
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

fn summary_line(s: &Summary) -> String {
    let when = s.date.map(relative).unwrap_or_default();
    format!(
        "{when:>9}  {}  {}",
        clean_line(&s.from),
        clean_line(&s.subject)
    )
}

fn term_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse().ok())
        .unwrap_or(100)
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
    fn truncation_and_sizes() {
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 4), "abc");
        assert_eq!(human_size(2048), "2.0 KB");
    }
}
