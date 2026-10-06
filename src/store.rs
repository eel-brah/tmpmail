//! Local state: the inboxes we created and which one is active. Credentials
//! live here, so the directory is 0700 and the file 0600, written atomically.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::provider::Inbox;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub inboxes: Vec<Inbox>,
}

pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Store { dir }
    }

    /// `$TMPMAIL_STATE_DIR`, else `$XDG_STATE_HOME/tmpmail` (or the platform
    /// equivalent). Never /tmp: other users could read the credentials.
    pub fn default_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("TMPMAIL_STATE_DIR") {
            return PathBuf::from(dir);
        }
        dirs::state_dir()
            .or_else(dirs::data_local_dir)
            .unwrap_or_else(|| PathBuf::from("."))
            .join("tmpmail")
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    pub fn load(&self) -> Result<State> {
        match fs::read_to_string(self.path()) {
            Ok(s) => serde_json::from_str(&s)
                .with_context(|| format!("corrupt state file {}", self.path().display()))
                .map_err(Error::from),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(anyhow::Error::from(e)
                .context(format!("cannot read {}", self.path().display()))
                .into()),
        }
    }

    pub fn save(&self, state: &State) -> Result<()> {
        create_private_dir(&self.dir)?;
        let tmp = self.dir.join(format!(".state.json.{}", std::process::id()));
        let data = serde_json::to_vec_pretty(state).context("serialize state")?;
        write_private(&tmp, &data)?;
        fs::rename(&tmp, self.path()).context("replace state file")?;
        Ok(())
    }
}

impl State {
    pub fn find(&self, address: &str) -> Option<usize> {
        let address = address.to_lowercase();
        self.inboxes
            .iter()
            .position(|i| i.address.to_lowercase() == address)
            .or_else(|| {
                // Allow a unique label or local-part as shorthand.
                let hits: Vec<usize> = self
                    .inboxes
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| {
                        i.label
                            .as_deref()
                            .is_some_and(|l| l.eq_ignore_ascii_case(&address))
                            || i.local_part().eq_ignore_ascii_case(&address)
                    })
                    .map(|(n, _)| n)
                    .collect();
                (hits.len() == 1).then(|| hits[0])
            })
    }

    /// Index of the inbox a command should act on: the explicit one, else the
    /// active one, else the most recent.
    pub fn resolve(&self, explicit: Option<&str>) -> Result<usize> {
        if let Some(addr) = explicit {
            return self
                .find(addr)
                .ok_or_else(|| Error::NotFound(format!("no stored inbox matches {addr:?}")));
        }
        if let Some(active) = &self.active {
            if let Some(n) = self.find(active) {
                return Ok(n);
            }
        }
        if self.inboxes.is_empty() {
            return Err(Error::NoInbox);
        }
        Ok(self.inboxes.len() - 1)
    }

    pub fn add(&mut self, inbox: Inbox) {
        self.inboxes
            .retain(|i| !i.address.eq_ignore_ascii_case(&inbox.address));
        self.active = Some(inbox.address.clone());
        self.inboxes.push(inbox);
    }

    pub fn remove(&mut self, n: usize) -> Inbox {
        let inbox = self.inboxes.remove(n);
        if self.active.as_deref() == Some(inbox.address.as_str()) {
            self.active = self.inboxes.last().map(|i| i.address.clone());
        }
        inbox
    }
}

pub fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod {}", dir.display()))?;
    }
    Ok(())
}

pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    f.write_all(data)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inbox(addr: &str) -> Inbox {
        Inbox::new("mailtm", addr.into(), json!({"password":"pw"}))
    }

    #[test]
    fn round_trip_with_private_permissions() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().join("state"));
        let mut state = store.load().unwrap();
        assert!(state.inboxes.is_empty());
        state.add(inbox("a@x.com"));
        store.save(&state).unwrap();

        let loaded = store.load().unwrap();
        assert_eq!(loaded.inboxes, state.inboxes);
        assert_eq!(loaded.active.as_deref(), Some("a@x.com"));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(store.dir()), 0o700);
            assert_eq!(mode(&store.dir().join("state.json")), 0o600);
        }
    }

    #[test]
    fn resolves_explicit_label_active_and_latest() {
        let mut s = State::default();
        assert!(matches!(s.resolve(None), Err(Error::NoInbox)));
        s.add(inbox("one@x.com"));
        let mut two = inbox("two@y.com");
        two.label = Some("github".into());
        s.add(two);
        assert_eq!(s.resolve(None).unwrap(), 1);
        assert_eq!(s.resolve(Some("ONE@x.com")).unwrap(), 0);
        assert_eq!(s.resolve(Some("github")).unwrap(), 1);
        assert_eq!(s.resolve(Some("one")).unwrap(), 0);
        assert!(s.resolve(Some("nope")).is_err());

        s.active = Some("one@x.com".into());
        assert_eq!(s.resolve(None).unwrap(), 0);
        s.remove(0);
        assert_eq!(s.active.as_deref(), Some("two@y.com"));
    }

    #[test]
    fn re_adding_an_address_replaces_it() {
        let mut s = State::default();
        s.add(inbox("a@x.com"));
        s.add(inbox("A@x.com"));
        assert_eq!(s.inboxes.len(), 1);
    }
}
