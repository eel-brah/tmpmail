//! Scheduling for the polling loops in `wait` and `watch`. Each inbox is
//! listed every `interval`; when its provider has a push stream (mail.tm,
//! DropMail), the stream runs on its own thread and wakes the loop as soon as
//! mail arrives, and polling slows to a safety net while it is connected.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use crate::provider::{Inbox, Provider, PushEvent};

/// How often an inbox with a live push stream is still listed, in case the
/// stream misses something.
const SAFETY_POLL: Duration = Duration::from_secs(60);
/// Longest wait before reconnecting a stream that keeps failing.
const MAX_BACKOFF: Duration = Duration::from_secs(300);
/// A stream that stayed up this long counts as healthy when it closes.
const STABLE: Duration = Duration::from_secs(60);

pub struct Poller {
    interval: Duration,
    push: bool,
    /// Next time each inbox (by address) should be listed.
    due: HashMap<String, Instant>,
    /// Inboxes a stream announced mail for since they were last listed.
    woken: HashSet<String>,
    streams: HashMap<String, Stream>,
    tx: Sender<Signal>,
    rx: Receiver<Signal>,
    next_id: u64,
}

struct Stream {
    /// Tells signals from this stream apart from those of an earlier one.
    id: u64,
    running: bool,
    live: bool,
    started: Instant,
    failures: u32,
    retry_at: Instant,
    /// The provider has no push for this inbox; don't ask again.
    unsupported: bool,
    stop: Arc<AtomicBool>,
}

struct Signal {
    address: String,
    id: u64,
    kind: Kind,
}

enum Kind {
    Connected,
    Mail,
    Closed,
}

impl Poller {
    pub fn new(interval: Duration, push: bool) -> Self {
        let (tx, rx) = channel();
        Poller {
            interval,
            push,
            due: HashMap::new(),
            woken: HashSet::new(),
            streams: HashMap::new(),
            tx,
            rx,
            next_id: 0,
        }
    }

    /// Whether `address` should be listed now.
    pub fn is_due(&self, address: &str) -> bool {
        self.woken.contains(address) || self.due.get(address).is_none_or(|t| Instant::now() >= *t)
    }

    /// Records that `address` was just listed (or failed to be) and when to
    /// list it next. After a push wake-up it is listed once more soon after,
    /// in case the provider announced the mail before it could be listed.
    pub fn polled(&mut self, address: &str, ok: bool) {
        let woken = self.woken.remove(address);
        let wait = if ok && !woken && self.is_live(address) {
            SAFETY_POLL
        } else {
            self.interval + Duration::from_millis(rand::random_range(0..1000))
        };
        self.due.insert(address.to_string(), Instant::now() + wait);
    }

    pub fn is_live(&self, address: &str) -> bool {
        self.streams.get(address).is_some_and(|s| s.live)
    }

    /// Starts a push stream for `inbox` unless one is running, is backing
    /// off after failures, or the provider has none. Called after each
    /// successful listing, so a stream that died is restarted with the
    /// inbox's current credentials.
    pub fn subscribe(&mut self, p: &dyn Provider, inbox: &Inbox) {
        if !self.push {
            return;
        }
        let now = Instant::now();
        let failures = match self.streams.get(&inbox.address) {
            Some(s) if s.unsupported || s.running || now < s.retry_at => return,
            Some(s) => s.failures,
            None => 0,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let Some(stream) = p.push(inbox) else {
            self.streams.insert(
                inbox.address.clone(),
                Stream {
                    id: 0,
                    running: false,
                    live: false,
                    started: now,
                    failures,
                    retry_at: now,
                    unsupported: true,
                    stop,
                },
            );
            return;
        };
        self.next_id += 1;
        let id = self.next_id;
        let (tx, address, flag) = (self.tx.clone(), inbox.address.clone(), stop.clone());
        std::thread::spawn(move || {
            let send = |kind| {
                let _ = tx.send(Signal {
                    address: address.clone(),
                    id,
                    kind,
                });
            };
            // Errors just close the stream; the poller decides when to retry.
            let _ = stream.run(&mut |event| {
                match event {
                    PushEvent::Connected => send(Kind::Connected),
                    PushEvent::Mail => send(Kind::Mail),
                    PushEvent::Alive => {}
                }
                !flag.load(Ordering::Relaxed)
            });
            send(Kind::Closed);
        });
        self.streams.insert(
            inbox.address.clone(),
            Stream {
                id,
                running: true,
                live: false,
                started: now,
                failures,
                retry_at: now,
                unsupported: false,
                stop,
            },
        );
    }

    /// Stops following `address` (it was dropped or rotated away).
    pub fn forget(&mut self, address: &str) {
        self.due.remove(address);
        self.woken.remove(address);
        if let Some(s) = self.streams.remove(address) {
            // The thread notices at its next event and exits.
            s.stop.store(true, Ordering::Relaxed);
        }
    }

    /// Sleeps until the next inbox is due, a stream announces mail, or
    /// `deadline` passes, whichever comes first.
    pub fn sleep(&mut self, deadline: Option<Instant>) {
        let now = Instant::now();
        let mut until = self
            .due
            .values()
            .min()
            .copied()
            .unwrap_or(now + self.interval);
        if let Some(d) = deadline {
            until = until.min(d);
        }
        if !self.woken.is_empty() {
            until = now;
        }
        // `self.tx` keeps the channel open, so this only ever times out.
        let first = self.rx.recv_timeout(until.saturating_duration_since(now));
        let signals: Vec<Signal> = first.into_iter().chain(self.rx.try_iter()).collect();
        for signal in signals {
            self.handle(signal);
        }
    }

    fn handle(&mut self, signal: Signal) {
        let Some(s) = self
            .streams
            .get_mut(&signal.address)
            .filter(|s| s.id == signal.id)
        else {
            return; // from a stream that was replaced or forgotten
        };
        let now = Instant::now();
        match signal.kind {
            Kind::Connected => {
                s.live = true;
                s.started = now;
            }
            Kind::Mail => {
                self.woken.insert(signal.address);
            }
            Kind::Closed => {
                let stable = s.live && now.duration_since(s.started) >= STABLE;
                s.failures = if stable { 1 } else { s.failures + 1 };
                s.running = false;
                s.live = false;
                s.retry_at = now + backoff(s.failures);
                // Back to normal polling until a stream is up again.
                let soon = now + self.interval;
                self.due
                    .entry(signal.address)
                    .and_modify(|t| *t = (*t).min(soon));
            }
        }
    }
}

impl Drop for Poller {
    /// Streams don't end with their poller on their own, so a long-running
    /// `tmpmail mcp` would keep one connection open per finished wait.
    fn drop(&mut self) {
        for s in self.streams.values() {
            s.stop.store(true, Ordering::Relaxed);
        }
    }
}

/// 4 s, 8 s, 16 s, ... up to five minutes.
fn backoff(failures: u32) -> Duration {
    Duration::from_secs(2u64 << failures.min(8)).min(MAX_BACKOFF)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result;
    use crate::provider::{Capabilities, CreateOpts, Message, PushStream, Summary};

    /// A provider whose push stream announces `mails` mails and closes.
    struct Fake {
        mails: usize,
    }

    struct FakeStream {
        mails: usize,
    }

    impl PushStream for FakeStream {
        fn run(self: Box<Self>, event: &mut dyn FnMut(PushEvent) -> bool) -> Result<()> {
            event(PushEvent::Connected);
            for _ in 0..self.mails {
                event(PushEvent::Mail);
            }
            Ok(())
        }
    }

    impl Provider for Fake {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn name(&self) -> &'static str {
            "Fake"
        }
        fn website(&self) -> &'static str {
            ""
        }
        fn caps(&self) -> Capabilities {
            unimplemented!()
        }
        fn domains(&self) -> Result<Vec<String>> {
            unimplemented!()
        }
        fn create(&self, _: &CreateOpts) -> Result<Inbox> {
            unimplemented!()
        }
        fn list(&self, _: &mut Inbox) -> Result<Vec<Summary>> {
            unimplemented!()
        }
        fn get(&self, _: &mut Inbox, _: &str) -> Result<Message> {
            unimplemented!()
        }
        fn push(&self, _: &Inbox) -> Option<Box<dyn PushStream>> {
            Some(Box::new(FakeStream { mails: self.mails }))
        }
    }

    fn inbox() -> Inbox {
        Inbox::new("fake", "a@x.com".into(), serde_json::Value::Null)
    }

    /// Handles signals until the stream for a@x.com has closed.
    fn drain(poller: &mut Poller) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while poller.streams["a@x.com"].running && Instant::now() < deadline {
            poller.sleep(Some(Instant::now() + Duration::from_millis(50)));
        }
    }

    #[test]
    fn push_wakes_the_inbox_and_failures_back_off() {
        let mut poller = Poller::new(Duration::from_secs(5), true);
        let inbox = inbox();
        poller.polled(&inbox.address, true);
        assert!(!poller.is_due(&inbox.address));

        poller.subscribe(&Fake { mails: 1 }, &inbox);
        drain(&mut poller);
        assert!(poller.is_due(&inbox.address), "mail wakes the inbox");

        // The stream closed right after connecting: retry later, not now.
        let s = &poller.streams[&inbox.address];
        assert!(!s.live);
        assert_eq!(s.failures, 1);
        let retry_at = s.retry_at;
        assert!(retry_at > Instant::now());
        poller.subscribe(&Fake { mails: 0 }, &inbox);
        assert_eq!(poller.streams[&inbox.address].retry_at, retry_at);
    }

    #[test]
    fn disabled_push_starts_nothing() {
        let mut poller = Poller::new(Duration::from_secs(5), false);
        poller.subscribe(&Fake { mails: 1 }, &inbox());
        assert!(poller.streams.is_empty());
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff(1), Duration::from_secs(4));
        assert_eq!(backoff(2), Duration::from_secs(8));
        assert_eq!(backoff(30), MAX_BACKOFF);
    }
}
