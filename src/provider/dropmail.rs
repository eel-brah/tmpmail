//! DropMail.me GraphQL API: https://dropmail.me/api/
//! Needs a free `af_` token (one POST, no signup). Sessions die after idle
//! time; the stored `restoreKey` brings the same address back in a new session.

use chrono::{Duration, Utc};
use serde_json::{Value, json};

use super::{
    Attachment, Capabilities, CreateOpts, Inbox, Message, Provider, Summary, base_url,
    domain_unavailable, opt_str, parse_date, str_field, with_token,
};
use std::path::PathBuf;

use crate::error::{Error, Result, message_not_found, unsupported};
use crate::http::Http;
use crate::store::{Store, create_private_dir, write_private};

const BASE: &str = "https://dropmail.me";
/// Longer lifetimes cost "credits" and trigger a captcha; 1d is free.
const TOKEN_LIFETIME: &str = "1d";
const TOKEN_REUSE_HOURS: i64 = 23;

const MAILS_QUERY: &str = "query($id: ID!) { session(id: $id) { expiresAt mails { \
    id fromAddr toAddr receivedAt headerFrom headerSubject text html downloadUrl \
    attachments { id name mime rawSize downloadUrl } } } }";

pub struct DropMail {
    base: String,
    http: Http,
    /// Where the shared API token is cached. Generating one per call gets the
    /// IP challenged with a captcha quickly.
    cache: Option<PathBuf>,
}

impl DropMail {
    pub fn new() -> Self {
        DropMail {
            base: base_url("dropmail", BASE),
            http: Http::new("dropmail"),
            cache: Some(Store::default_dir().join("dropmail-token.json")),
        }
    }

    /// `TMPMAIL_DROPMAIL_TOKEN`, else a cached token younger than a day,
    /// else a freshly generated one (cached for next time).
    fn token(&self, refresh: bool) -> Result<String> {
        if let Ok(t) = std::env::var("TMPMAIL_DROPMAIL_TOKEN") {
            return Ok(t);
        }
        if !refresh {
            if let Some(t) = self.cached_token() {
                return Ok(t);
            }
        }
        let token = self.new_token()?;
        if let Some(path) = &self.cache {
            let data = json!({ "token": token, "created_at": Utc::now() }).to_string();
            if let Some(dir) = path.parent() {
                let _ = create_private_dir(dir);
            }
            let _ = write_private(path, data.as_bytes());
        }
        Ok(token)
    }

    fn cached_token(&self) -> Option<String> {
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(self.cache.as_ref()?).ok()?).ok()?;
        let created = parse_date(&v["created_at"])?;
        (Utc::now() - created < Duration::hours(TOKEN_REUSE_HOURS))
            .then(|| str_field(&v, "token").to_string())
            .filter(|t| !t.is_empty())
    }

    fn new_token(&self) -> Result<String> {
        let v: Value = match self.http.json(|| {
            self.http
                .client()
                .post(format!("{}/api/token/generate", self.base))
                .json(&json!({ "type": "af", "lifetime": TOKEN_LIFETIME }))
        }) {
            Err(Error::BadResponse {
                status: Some(402), ..
            }) => {
                return Err(self.http.bad(
                    "DropMail wants a captcha before issuing more API tokens to this IP; \
                     try later or set TMPMAIL_DROPMAIL_TOKEN (see https://dropmail.me/api/)",
                ));
            }
            other => other?,
        };
        match str_field(&v, "token") {
            "" => Err(self.http.bad("token missing from /api/token/generate")),
            t => Ok(t.to_string()),
        }
    }

    fn gql(&self, token: &str, query: &str, variables: &Value) -> Result<Value> {
        let url = format!("{}/api/graphql/{token}", self.base);
        self.http.graphql(&url, query, variables)
    }

    /// Ensures a valid token, regenerating it if the API rejects the stored one.
    fn gql_inbox(&self, inbox: &mut Inbox, query: &str, variables: Value) -> Result<Value> {
        let refresh = |inbox: &mut Inbox| {
            let token = self.token(true)?;
            inbox.set_cred("token", token.clone());
            Ok(token)
        };
        with_token(inbox, refresh, |token| self.gql(token, query, &variables))
    }

    /// Returns the session's mails, restoring the address into a fresh session
    /// when the old one expired.
    fn mails(&self, inbox: &mut Inbox) -> Result<Vec<Value>> {
        let session = inbox.cred_str("session_id").unwrap_or_default().to_string();
        let mut data = self.gql_inbox(inbox, MAILS_QUERY, json!({ "id": session }))?;
        let session = &mut data["session"];
        if session.is_object() {
            if let Some(exp) = parse_date(&session["expiresAt"]) {
                inbox.expires_at = Some(exp);
            }
            return Ok(match session["mails"].take() {
                Value::Array(mails) => mails,
                _ => Vec::new(),
            });
        }
        self.restore(inbox)?;
        Ok(Vec::new())
    }

    fn find_mail(&self, inbox: &mut Inbox, id: &str) -> Result<Value> {
        self.mails(inbox)?
            .into_iter()
            .find(|m| str_field(m, "id") == id)
            .ok_or_else(|| message_not_found(id))
    }

    fn restore(&self, inbox: &mut Inbox) -> Result<()> {
        let key = inbox
            .cred_str("restore_key")
            .ok_or_else(|| Error::Expired(inbox.address.clone()))?
            .to_string();
        let data = self.gql_inbox(
            inbox,
            "mutation { introduceSession(input: {withAddress: false}) { id expiresAt } }",
            json!({}),
        )?;
        let session = str_field(&data["introduceSession"], "id").to_string();
        let exp = parse_date(&data["introduceSession"]["expiresAt"]);
        let restored = self.gql_inbox(
            inbox,
            "mutation($s: ID!, $a: String!, $k: String!) { restoreAddress(input: \
             {sessionId: $s, mailAddress: $a, restoreKey: $k}) { address } }",
            json!({ "s": session, "a": inbox.address, "k": key }),
        )?;
        if restored["restoreAddress"].is_null() {
            return Err(Error::Expired(inbox.address.clone()));
        }
        inbox.set_cred("session_id", session);
        inbox.expires_at = exp;
        Ok(())
    }

    fn message(v: &Value) -> Message {
        let from = match str_field(v, "headerFrom") {
            "" => str_field(v, "fromAddr"),
            h => h,
        };
        Message {
            id: str_field(v, "id").to_string(),
            from: from.to_string(),
            to: vec![str_field(v, "toAddr").to_string()],
            subject: str_field(v, "headerSubject").to_string(),
            date: parse_date(&v["receivedAt"]),
            text: opt_str(v, "text"),
            html: opt_str(v, "html"),
            attachments: v["attachments"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|att| Attachment {
                            id: str_field(att, "id").to_string(),
                            filename: str_field(att, "name").to_string(),
                            content_type: str_field(att, "mime").to_string(),
                            size: att["rawSize"].as_u64(),
                            url: opt_str(att, "downloadUrl"),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

impl Provider for DropMail {
    fn id(&self) -> &'static str {
        "dropmail"
    }
    fn name(&self) -> &'static str {
        "DropMail.me"
    }
    fn website(&self) -> &'static str {
        "https://dropmail.me"
    }

    fn caps(&self) -> Capabilities {
        Capabilities {
            private: true,
            custom_user: false,
            choose_domain: true,
            attachments: true,
            raw: true,
            delete_message: false,
            delete_inbox: false,
            login: false,
            lifetime: "session renewed on each check; address restorable later",
        }
    }

    fn domains(&self) -> Result<Vec<String>> {
        Ok(self
            .domain_ids()?
            .into_iter()
            .map(|(_, name)| name)
            .collect())
    }

    fn create(&self, opts: &CreateOpts) -> Result<Inbox> {
        if opts.user.is_some() {
            return Err(unsupported(self.id(), "choosing the username"));
        }
        let domain_id = match &opts.domain {
            None => None,
            Some(d) => {
                let (ids, names): (Vec<_>, Vec<_>) = self.domain_ids()?.into_iter().unzip();
                match names.iter().position(|name| name == d) {
                    Some(k) => Some(ids[k].clone()),
                    None => return Err(domain_unavailable(self.id(), d, &names)),
                }
            }
        };
        let token = self.token(false)?;
        let data = self.gql(
            &token,
            "mutation($d: ID) { introduceSession(input: {withAddress: true, domainId: $d}) { \
             id expiresAt addresses { address restoreKey } } }",
            &json!({ "d": domain_id }),
        )?;
        let s = &data["introduceSession"];
        let addr = &s["addresses"][0];
        let address = str_field(addr, "address");
        if address.is_empty() {
            return Err(self.http.bad("no address in introduceSession response"));
        }
        let mut inbox = Inbox::new(
            self.id(),
            address.to_string(),
            json!({
                "token": token,
                "session_id": str_field(s, "id"),
                "restore_key": str_field(addr, "restoreKey"),
            }),
        );
        inbox.expires_at = parse_date(&s["expiresAt"]).or(Some(Utc::now() + Duration::minutes(10)));
        Ok(inbox)
    }

    fn list(&self, inbox: &mut Inbox) -> Result<Vec<Summary>> {
        Ok(self
            .mails(inbox)?
            .iter()
            .map(|m| Self::message(m).summary())
            .collect())
    }

    fn get(&self, inbox: &mut Inbox, id: &str) -> Result<Message> {
        Ok(Self::message(&self.find_mail(inbox, id)?))
    }

    fn raw(&self, inbox: &mut Inbox, id: &str) -> Result<Vec<u8>> {
        let mail = self.find_mail(inbox, id)?;
        let url = str_field(&mail, "downloadUrl").to_string();
        self.http.bytes(|| self.http.client().get(&url))
    }

    fn attachment(&self, _inbox: &mut Inbox, _msg: &Message, att: &Attachment) -> Result<Vec<u8>> {
        let url = att
            .url
            .clone()
            .ok_or_else(|| self.http.bad("attachment has no download URL"))?;
        self.http.bytes(|| self.http.client().get(&url))
    }
}

impl DropMail {
    fn domain_ids(&self) -> Result<Vec<(String, String)>> {
        let token = self.token(false)?;
        let data = self.gql(&token, "{ domains { id name availableVia } }", &json!({}))?;
        let list: Vec<(String, String)> = data["domains"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|d| {
                d["availableVia"]
                    .as_array()
                    .is_none_or(|via| via.iter().any(|v| v == "API"))
            })
            .map(|d| {
                (
                    str_field(d, "id").to_string(),
                    str_field(d, "name").to_string(),
                )
            })
            .collect();
        if list.is_empty() {
            return Err(self.http.bad("no domains"));
        }
        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    #[test]
    fn create_then_restore_expired_session() {
        let mut s = Server::new();
        s.mock("POST", "/api/token/generate")
            .with_body(r#"{"token":"af_tok"}"#)
            .create();
        s.mock("POST", "/api/graphql/af_tok")
            .match_body(Matcher::Regex(
                "introduceSession\\(input: \\{withAddress: true".into(),
            ))
            .with_body(
                r#"{"data":{"introduceSession":{"id":"S1","expiresAt":"2030-01-01T00:00:00+00:00",
                "addresses":[{"address":"abc@10mail.org","restoreKey":"rk"}]}}}"#,
            )
            .create();
        let p = DropMail {
            base: s.url(),
            http: Http::new("dropmail"),
            cache: None,
        };
        let mut inbox = p.create(&CreateOpts::default()).unwrap();
        assert_eq!(inbox.address, "abc@10mail.org");
        assert_eq!(inbox.cred_str("restore_key"), Some("rk"));

        // Session gone: list must restore the address into a new session.
        s.mock("POST", "/api/graphql/af_tok")
            .match_body(Matcher::Regex("session\\(id".into()))
            .with_body(r#"{"data":{"session":null}}"#)
            .create();
        s.mock("POST", "/api/graphql/af_tok")
            .match_body(Matcher::Regex("withAddress: false".into()))
            .with_body(r#"{"data":{"introduceSession":{"id":"S2","expiresAt":"2030-01-01T00:00:00+00:00"}}}"#)
            .create();
        s.mock("POST", "/api/graphql/af_tok")
            .match_body(Matcher::Regex("restoreAddress".into()))
            .with_body(r#"{"data":{"restoreAddress":{"address":"abc@10mail.org"}}}"#)
            .create();
        assert!(p.list(&mut inbox).unwrap().is_empty());
        assert_eq!(inbox.cred_str("session_id"), Some("S2"));
    }

    #[test]
    fn graphql_errors_are_surfaced() {
        let mut s = Server::new();
        s.mock("POST", "/api/token/generate")
            .with_body(r#"{"token":"af_tok"}"#)
            .create();
        s.mock("POST", "/api/graphql/af_tok")
            .with_body(r#"{"errors":[{"message":"RATE_LIMIT_EXCEEDED"}]}"#)
            .create();
        let p = DropMail {
            base: s.url(),
            http: Http::new("dropmail"),
            cache: None,
        };
        assert!(matches!(
            p.domains().unwrap_err(),
            Error::RateLimited { .. }
        ));
    }
}
