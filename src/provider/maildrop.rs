//! Maildrop GraphQL API: https://docs.maildrop.cc
//! PUBLIC inboxes: anyone who knows the name can read them. New senders are
//! greylisted, so the first message from a server can arrive minutes late.

use chrono::{Duration, Utc};
use serde_json::{Value, json};

use super::{
    Capabilities, CreateOpts, Inbox, Message, Provider, Summary, parse_date, random_user, str_field,
};
use crate::error::{Error, Result};
use crate::http::Http;

const BASE: &str = "https://api.maildrop.cc/graphql";
const DOMAIN: &str = "maildrop.cc";

pub struct Maildrop {
    base: String,
    http: Http,
}

impl Maildrop {
    pub fn new() -> Self {
        Maildrop {
            base: std::env::var("TMPMAIL_MAILDROP_URL").unwrap_or_else(|_| BASE.to_string()),
            http: Http::new("maildrop"),
        }
    }

    fn gql(&self, query: &str, variables: Value) -> Result<Value> {
        let body = json!({ "query": query, "variables": variables });
        let v: Value = self
            .http
            .json(|| self.http.client().post(&self.base).json(&body))?;
        if let Some(errors) = v
            .get("errors")
            .and_then(Value::as_array)
            .filter(|e| !e.is_empty())
        {
            let msg: Vec<&str> = errors.iter().map(|e| str_field(e, "message")).collect();
            return Err(self.http.bad(format!("GraphQL error: {}", msg.join("; "))));
        }
        Ok(v["data"].clone())
    }
}

impl Provider for Maildrop {
    fn id(&self) -> &'static str {
        "maildrop"
    }
    fn name(&self) -> &'static str {
        "Maildrop"
    }
    fn website(&self) -> &'static str {
        "https://maildrop.cc"
    }

    fn caps(&self) -> Capabilities {
        Capabilities {
            private: false,
            custom_user: true,
            choose_domain: false,
            attachments: false,
            raw: true,
            delete_message: true,
            delete_inbox: false,
            login: true,
            lifetime: "wiped after 24 h without mail; max 10 messages",
        }
    }

    fn domains(&self) -> Result<Vec<String>> {
        Ok(vec![DOMAIN.to_string()])
    }

    fn health(&self) -> Result<()> {
        self.gql("{ status }", json!({})).map(|_| ())
    }

    fn create(&self, opts: &CreateOpts) -> Result<Inbox> {
        if opts.domain.as_deref().is_some_and(|d| d != DOMAIN) {
            return Err(Error::Usage(format!(
                "maildrop: only {DOMAIN} is available"
            )));
        }
        let user = opts.user.clone().unwrap_or_else(random_user).to_lowercase();
        let mut inbox = Inbox::new("maildrop", format!("{user}@{DOMAIN}"), json!({}));
        inbox.expires_at = Some(Utc::now() + Duration::hours(24));
        Ok(inbox)
    }

    /// Public inboxes need no password: "logging in" is just picking the name.
    fn login(&self, address: &str, _password: &str) -> Result<Inbox> {
        let user = address.split('@').next().unwrap_or(address);
        self.create(&CreateOpts {
            user: Some(user.to_string()),
            ..Default::default()
        })
    }

    fn list(&self, inbox: &mut Inbox) -> Result<Vec<Summary>> {
        let data = self.gql(
            "query($m: String!) { inbox(mailbox: $m) { id headerfrom subject date } }",
            json!({ "m": inbox.local_part() }),
        )?;
        Ok(data["inbox"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|m| Summary {
                id: str_field(m, "id").to_string(),
                from: str_field(m, "headerfrom").to_string(),
                subject: str_field(m, "subject").to_string(),
                intro: String::new(),
                date: parse_date(&m["date"]),
                seen: false,
                has_attachments: false,
            })
            .collect())
    }

    fn get(&self, inbox: &mut Inbox, id: &str) -> Result<Message> {
        let data = self.gql(
            "query($m: String!, $id: String!) { message(mailbox: $m, id: $id) { \
             id headerfrom subject date html } }",
            json!({ "m": inbox.local_part(), "id": id }),
        )?;
        let m = &data["message"];
        if m.is_null() {
            return Err(Error::NotFound(format!("message {id} not found")));
        }
        Ok(Message {
            id: str_field(m, "id").to_string(),
            from: str_field(m, "headerfrom").to_string(),
            to: vec![inbox.address.clone()],
            subject: str_field(m, "subject").to_string(),
            date: parse_date(&m["date"]),
            text: None,
            html: m["html"]
                .as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            attachments: Vec::new(),
        })
    }

    fn raw(&self, inbox: &mut Inbox, id: &str) -> Result<Vec<u8>> {
        let data = self.gql(
            "query($m: String!, $id: String!) { message(mailbox: $m, id: $id) { data } }",
            json!({ "m": inbox.local_part(), "id": id }),
        )?;
        data["message"]["data"]
            .as_str()
            .map(|s| s.as_bytes().to_vec())
            .ok_or_else(|| Error::NotFound(format!("message {id} not found")))
    }

    fn delete_message(&self, inbox: &mut Inbox, id: &str) -> Result<()> {
        self.gql(
            "mutation($m: String!, $id: String!) { delete(mailbox: $m, id: $id) }",
            json!({ "m": inbox.local_part(), "id": id }),
        )
        .map(|_| ())
    }
}
