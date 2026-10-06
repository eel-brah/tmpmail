//! Optional config file plus environment overrides, for interactive use and CI.

use std::path::PathBuf;

use anyhow::Context;
use serde::Deserialize;

use crate::error::{Error, Result};
use crate::provider;

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Provider ids tried in order by `new`.
    pub providers: Option<Vec<String>>,
    pub browser: Option<String>,
    pub clipboard_cmd: Option<String>,
    /// Seconds between inbox checks in `wait` and `watch`.
    pub poll_interval: Option<u64>,
    /// Default `wait` timeout in seconds.
    pub timeout: Option<u64>,
}

impl Config {
    pub fn path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os("TMPMAIL_CONFIG") {
            return Some(PathBuf::from(p));
        }
        dirs::config_dir().map(|d| d.join("tmpmail").join("config.toml"))
    }

    pub fn load() -> Result<Self> {
        let mut cfg = match Self::path() {
            Some(path) => match std::fs::read_to_string(&path) {
                Ok(s) => toml::from_str(&s)
                    .with_context(|| format!("invalid config {}", path.display()))?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Config::default(),
                Err(e) => {
                    return Err(anyhow::Error::from(e)
                        .context(format!("cannot read {}", path.display()))
                        .into());
                }
            },
            None => Config::default(),
        };
        if let Ok(p) = std::env::var("TMPMAIL_PROVIDER") {
            cfg.providers = Some(p.split(',').map(|s| s.trim().to_string()).collect());
        }
        if let Some(t) = env_u64("TMPMAIL_TIMEOUT")? {
            cfg.timeout = Some(t);
        }
        if let Some(t) = env_u64("TMPMAIL_POLL_INTERVAL")? {
            cfg.poll_interval = Some(t);
        }
        if let Some(ids) = &cfg.providers {
            for id in ids {
                provider::get(id)?;
            }
        }
        Ok(cfg)
    }

    pub fn provider_order(&self) -> Vec<String> {
        match &self.providers {
            Some(p) if !p.is_empty() => p.clone(),
            _ => provider::DEFAULT_ORDER
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    /// Seconds between checks: the command-line value, else the config, at least 2.
    pub fn poll_interval(&self, cli: Option<u64>) -> u64 {
        cli.or(self.poll_interval).unwrap_or(5).max(2)
    }

    pub fn timeout(&self) -> u64 {
        self.timeout.unwrap_or(180)
    }
}

fn env_u64(key: &str) -> Result<Option<u64>> {
    match std::env::var(key) {
        Ok(v) => v
            .parse()
            .map(Some)
            .map_err(|_| Error::Usage(format!("{key} must be a number of seconds, got {v:?}"))),
        Err(_) => Ok(None),
    }
}
