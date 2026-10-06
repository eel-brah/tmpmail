//! Guerrilla Mail JSON API: https://www.guerrillamail.com/GuerrillaMailAPI.html
//! Session based (`sid_token`). Addresses keep mail for 60 minutes; any of the
//! Guerrilla domains delivers to the same mailbox.

use chrono::{Duration, Utc};
use serde_json::{Value, json};

use super::{Capabilities, CreateOpts, Inbox, Message, Provider, Summary, parse_date, str_field};
use crate::error::{Error, Result};
use crate::http::Http;

const BASE: &str = "https://api.guerrillamail.com/ajax.php";

/// Guerrilla has no domain-list endpoint; these are the domains its web UI
/// offers, all of which deliver to the same mailbox.
const DOMAINS: &[&str] = &[
    "sharklasers.com",
    "guerrillamail.info",
    "grr.la",
    "guerrillamail.biz",
    "guerrillamail.com",
    "guerrillamail.de",
    "guerrillamail.net",
    "guerrillamail.org",
    "guerrillamailblock.com",
    "pokemail.net",
    "spam4.me",
];

pub struct Guerrilla {
    base: String,
    http: Http,
}

impl Guerrilla {
    pub fn new() -> Self {
        Guerrilla {
            base: std::env::var("TMPMAIL_GUERRILLA_URL").unwrap_or_else(|_| BASE.to_string()),
            http: Http::new("guerrilla"),
        }
    }

    fn call(&self, f: &str, params: &[(&str, &str)]) -> Result<Value> {
        let mut query: Vec<(&str, &str)> =
            vec![("f", f), ("ip", "127.0.0.1"), ("agent", "tmpmail")];
        query.extend_from_slice(params);
        let v: Value = self
            .http
            .json(|| self.http.client().get(&self.base).query(&query))?;
        if let Some(err) = v.get("error").and_then(Value::as_str) {
            return Err(self.http.bad(err.to_string()));
        }
        Ok(v)
    }

    /// Calls with the inbox's session and re-binds the address if the API
    /// silently started a new session (it does after ~18 minutes idle).
    fn session_call(&self, inbox: &mut Inbox, f: &str, params: &[(&str, &str)]) -> Result<Value> {
        let sid = inbox.cred_str("sid_token").unwrap_or_default().to_string();
        let mut all = vec![("sid_token", sid.as_str())];
        all.extend_from_slice(params);
        let v = self.call(f, &all)?;
        if let Some(new_sid) = v.get("sid_token").and_then(Value::as_str) {
            if new_sid != sid {
                inbox.set_cred("sid_token", new_sid.to_string());
                let user = inbox.local_part().to_string();
                self.call(
                    "set_email_user",
                    &[
                        ("sid_token", new_sid),
                        ("email_user", &user),
                        ("lang", "en"),
                    ],
                )?;
                // The first call ran against an empty session; repeat it.
                let mut again = vec![("sid_token", new_sid)];
                again.extend_from_slice(params);
                return self.call(f, &again);
            }
        }
        Ok(v)
    }

    fn summary(v: &Value) -> Summary {
        Summary {
            id: id_of(&v["mail_id"]),
            from: str_field(v, "mail_from").to_string(),
            subject: decode_entities(str_field(v, "mail_subject")),
            intro: decode_entities(str_field(v, "mail_excerpt")),
            date: parse_date(&v["mail_timestamp"]),
            seen: v["mail_read"].as_i64().unwrap_or(0) != 0
                || v["mail_read"].as_str().is_some_and(|s| s != "0"),
            has_attachments: v["att"].as_i64().unwrap_or(0) > 0
                || v["att"].as_str().is_some_and(|s| s != "0"),
        }
    }
}

fn id_of(v: &Value) -> String {
    match v {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => String::new(),
    }
}

/// Guerrilla HTML-escapes subjects and excerpts.
fn decode_entities(s: &str) -> String {
    s.replace("&#039;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

impl Provider for Guerrilla {
    fn id(&self) -> &'static str {
        "guerrilla"
    }
    fn name(&self) -> &'static str {
        "Guerrilla Mail"
    }
    fn website(&self) -> &'static str {
        "https://www.guerrillamail.com"
    }

    fn caps(&self) -> Capabilities {
        Capabilities {
            private: true,
            custom_user: true,
            choose_domain: true,
            attachments: false,
            raw: false,
            delete_message: true,
            delete_inbox: true,
            login: false,
            lifetime: "mail kept 60 minutes",
        }
    }

    fn domains(&self) -> Result<Vec<String>> {
        Ok(DOMAINS.iter().map(|d| d.to_string()).collect())
    }

    fn health(&self) -> Result<()> {
        // Starts an anonymous session but no inbox is kept.
        self.call("get_email_address", &[("lang", "en")])
            .map(|_| ())
    }

    fn create(&self, opts: &CreateOpts) -> Result<Inbox> {
        if let Some(d) = &opts.domain {
            if !DOMAINS.contains(&d.as_str()) {
                return Err(Error::Usage(format!(
                    "guerrilla: domain {d:?} not available, choose one of: {}",
                    DOMAINS.join(", ")
                )));
            }
        }
        let mut v = self.call("get_email_address", &[("lang", "en")])?;
        let sid = str_field(&v, "sid_token").to_string();
        if let Some(user) = &opts.user {
            v = self.call(
                "set_email_user",
                &[("sid_token", &sid), ("email_user", user), ("lang", "en")],
            )?;
        }
        let address = str_field(&v, "email_addr");
        let Some((local, default_domain)) = address.split_once('@') else {
            return Err(self.http.bad("no email_addr in response"));
        };
        let domain = opts.domain.as_deref().unwrap_or(default_domain);
        let mut inbox = Inbox::new(
            "guerrilla",
            format!("{local}@{domain}"),
            json!({ "sid_token": str_field(&v, "sid_token").to_string() }),
        );
        if inbox.cred_str("sid_token") == Some("") {
            inbox.set_cred("sid_token", sid);
        }
        inbox.expires_at = Some(Utc::now() + Duration::minutes(60));
        Ok(inbox)
    }

    fn list(&self, inbox: &mut Inbox) -> Result<Vec<Summary>> {
        let v = self.session_call(inbox, "get_email_list", &[("offset", "0")])?;
        Ok(v["list"]
            .as_array()
            .into_iter()
            .flatten()
            .map(Self::summary)
            .collect())
    }

    fn get(&self, inbox: &mut Inbox, id: &str) -> Result<Message> {
        let v = self.session_call(inbox, "fetch_email", &[("email_id", id)])?;
        if v.is_null() || v == json!(false) || v.get("mail_id").is_none() {
            return Err(Error::NotFound(format!("message {id} not found")));
        }
        let s = Self::summary(&v);
        let body = str_field(&v, "mail_body").to_string();
        Ok(Message {
            id: s.id,
            from: s.from,
            to: vec![inbox.address.clone()],
            subject: s.subject,
            date: s.date,
            // Even plain-text mail comes back HTML-escaped inside <pre>.
            text: None,
            html: Some(body),
            attachments: Vec::new(),
        })
    }

    fn delete_message(&self, inbox: &mut Inbox, id: &str) -> Result<()> {
        self.session_call(inbox, "del_email", &[("email_ids[]", id)])
            .map(|_| ())
    }

    fn delete_inbox(&self, inbox: &mut Inbox) -> Result<()> {
        let addr = inbox.address.clone();
        self.session_call(inbox, "forget_me", &[("email_addr", &addr)])
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    #[test]
    fn create_custom_user_and_read() {
        let mut s = Server::new();
        s.mock("GET", "/")
            .match_query(Matcher::UrlEncoded("f".into(), "get_email_address".into()))
            .with_body(r#"{"email_addr":"rnd@guerrillamailblock.com","sid_token":"sid1"}"#)
            .create();
        s.mock("GET", "/")
            .match_query(Matcher::UrlEncoded("f".into(), "set_email_user".into()))
            .with_body(r#"{"email_addr":"otter@guerrillamailblock.com","sid_token":"sid1"}"#)
            .create();
        s.mock("GET", "/")
            .match_query(Matcher::UrlEncoded("f".into(), "get_email_list".into()))
            .with_body(
                r#"{"list":[{"mail_id":"7","mail_from":"x@y.z","mail_subject":"It&#039;s here",
                "mail_excerpt":"code 4242","mail_timestamp":"1791194910","mail_read":0,"att":"0"}],
                "sid_token":"sid1"}"#,
            )
            .create();
        s.mock("GET", "/")
            .match_query(Matcher::UrlEncoded("f".into(), "fetch_email".into()))
            .with_body(
                r#"{"mail_id":7,"mail_from":"x@y.z","mail_subject":"It&#039;s here",
                "mail_body":"<b>code 4242</b>","content_type":"text/html","mail_timestamp":1791194910,
                "sid_token":"sid1"}"#,
            )
            .create();

        let p = Guerrilla {
            base: s.url() + "/",
            http: Http::new("guerrilla"),
        };
        let mut inbox = p
            .create(&CreateOpts {
                user: Some("otter".into()),
                domain: Some("sharklasers.com".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(inbox.address, "otter@sharklasers.com");

        let list = p.list(&mut inbox).unwrap();
        assert_eq!(list[0].id, "7");
        assert_eq!(list[0].subject, "It's here");
        assert!(list[0].date.is_some());

        let msg = p.get(&mut inbox, "7").unwrap();
        assert_eq!(msg.html.as_deref(), Some("<b>code 4242</b>"));
        assert!(msg.text.is_none());
    }
}
