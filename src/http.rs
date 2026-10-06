use std::sync::OnceLock;
use std::thread::sleep;
use std::time::Duration;

use reqwest::StatusCode;
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::error::{Error, Result};
use crate::render::clean_line;

const USER_AGENT: &str = concat!("tmpmail/", env!("CARGO_PKG_VERSION"));
const MAX_RETRIES: u32 = 3;

/// One client for the whole process, so polling reuses pooled connections
/// instead of paying a TLS handshake per request.
fn shared_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        let timeout = std::env::var("TMPMAIL_HTTP_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(20);
        Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(timeout))
            .connect_timeout(Duration::from_secs(10))
            .https_only(!cfg!(test) && std::env::var_os("TMPMAIL_ALLOW_HTTP").is_none())
            .build()
            .expect("HTTP client configuration is valid")
    })
}

/// Thin wrapper around a blocking client that turns every failure into a typed
/// [`Error`] naming the provider, so a dead backend never surfaces as garbage.
pub struct Http {
    client: &'static Client,
    provider: &'static str,
}

impl Http {
    pub fn new(provider: &'static str) -> Self {
        Http {
            client: shared_client(),
            provider,
        }
    }

    pub fn client(&self) -> &Client {
        self.client
    }

    /// Sends a request, retrying on 429 (honouring Retry-After) and mapping
    /// transport errors and non-2xx statuses to typed errors.
    pub fn send(&self, build: impl Fn() -> RequestBuilder) -> Result<Response> {
        let mut attempt = 0;
        loop {
            let resp = build().send().map_err(|e| self.transport_error(e))?;
            let status = resp.status();
            if status == StatusCode::TOO_MANY_REQUESTS && attempt < MAX_RETRIES {
                attempt += 1;
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(1 << attempt)
                    .min(30);
                sleep(Duration::from_secs(wait));
                continue;
            }
            if status.is_success() {
                return Ok(resp);
            }
            let body = resp.text().unwrap_or_default();
            return Err(self.status_error(status, &body));
        }
    }

    pub fn json<T: DeserializeOwned>(&self, build: impl Fn() -> RequestBuilder) -> Result<T> {
        let resp = self.send(build)?;
        let body = resp.text().map_err(|e| self.transport_error(e))?;
        self.parse(&body)
    }

    pub fn bytes(&self, build: impl Fn() -> RequestBuilder) -> Result<Vec<u8>> {
        let resp = self.send(build)?;
        resp.bytes()
            .map(|b| b.to_vec())
            .map_err(|e| self.transport_error(e))
    }

    /// POSTs a GraphQL query and returns its `data`, turning a non-empty
    /// `errors` array into a typed error.
    pub fn graphql(&self, url: &str, query: &str, variables: &Value) -> Result<Value> {
        let body = json!({ "query": query, "variables": variables });
        let mut v: Value = self.json(|| self.client.post(url).json(&body))?;
        if let Some(errors) = v
            .get("errors")
            .and_then(Value::as_array)
            .filter(|e| !e.is_empty())
        {
            let msg = errors
                .iter()
                .map(|e| e.get("message").and_then(Value::as_str).unwrap_or_default())
                .collect::<Vec<_>>()
                .join("; ");
            if msg.contains("RATE_LIMIT") {
                return Err(Error::RateLimited {
                    provider: self.provider.to_string(),
                });
            }
            return Err(self.bad(format!("GraphQL error: {msg}")));
        }
        Ok(v.get_mut("data").map(Value::take).unwrap_or_default())
    }

    fn parse<T: DeserializeOwned>(&self, body: &str) -> Result<T> {
        serde_json::from_str(body).map_err(|e| {
            let trimmed = body.trim_start();
            let detail = if trimmed.is_empty() {
                "empty body".to_string()
            } else if trimmed.starts_with('<') {
                "got an HTML page instead of JSON, the API may be down or blocked".to_string()
            } else {
                format!("invalid JSON: {e}; body starts with {:?}", snippet(body))
            };
            self.bad(detail)
        })
    }

    pub fn bad(&self, detail: impl Into<String>) -> Error {
        Error::BadResponse {
            provider: self.provider.to_string(),
            detail: detail.into(),
            status: None,
        }
    }

    fn transport_error(&self, e: reqwest::Error) -> Error {
        let detail = if e.is_timeout() {
            "request timed out".to_string()
        } else if e.is_connect() {
            format!("connection failed: {}", root_cause(&e))
        } else {
            root_cause(&e)
        };
        Error::Unreachable {
            provider: self.provider.to_string(),
            detail,
        }
    }

    fn status_error(&self, status: StatusCode, body: &str) -> Error {
        let provider = self.provider.to_string();
        let code = status.as_u16();
        let detail = format!("HTTP {code}{}", describe_body(body));
        match code {
            401 | 403 => Error::Auth { provider, detail },
            404 => Error::NotFound(format!("{provider}: not found ({detail})")),
            429 => Error::RateLimited { provider },
            500..=599 => Error::Unreachable { provider, detail },
            _ => Error::BadResponse {
                provider,
                detail,
                status: Some(code),
            },
        }
    }
}

fn root_cause(e: &(dyn std::error::Error + 'static)) -> String {
    let mut cur = e;
    while let Some(next) = cur.source() {
        cur = next;
    }
    cur.to_string()
}

fn describe_body(body: &str) -> String {
    let t = body.trim();
    if t.is_empty() || t.starts_with('<') {
        return String::new();
    }
    // Hydra/JSON APIs put a human message in one of these fields.
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(t) {
        for key in ["hydra:description", "detail", "message", "error"] {
            if let Some(s) = v.get(key).and_then(|s| s.as_str()) {
                return format!(": {}", clean_line(s));
            }
        }
    }
    format!(": {}", snippet(t))
}

fn snippet(s: &str) -> String {
    clean_line(&s.chars().take(120).collect::<String>())
}
