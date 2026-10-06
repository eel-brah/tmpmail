//! tempmail.lol v2 free API: https://github.com/tempmail-lol/server/wiki/v2-API-Endpoints
//! One POST to create, one GET to poll. Free inboxes live one hour and their
//! messages carry no id, so ids are derived from their content.

use chrono::{Duration, Utc};
use serde_json::{Value, json};

use super::{
    Capabilities, CreateOpts, Inbox, Message, Provider, Summary, base_url, opt_str, parse_date,
    str_field, synthetic_id,
};
use crate::error::{Error, Result, message_not_found, unsupported};
use crate::http::Http;

const BASE: &str = "https://api.tempmail.lol/v2";

pub struct TempmailLol {
    base: String,
    http: Http,
}

impl TempmailLol {
    pub fn new() -> Self {
        TempmailLol {
            base: base_url("tempmail_lol", BASE),
            http: Http::new("tempmail_lol"),
        }
    }

    fn emails(&self, inbox: &Inbox) -> Result<Vec<Message>> {
        let token = inbox.cred_str("token").unwrap_or_default();
        let v: Value = self.http.json(|| {
            self.http
                .client()
                .get(format!("{}/inbox", self.base))
                .query(&[("token", token)])
        })?;
        if v["expired"].as_bool() == Some(true) {
            return Err(Error::Expired(inbox.address.clone()));
        }
        Ok(v["emails"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| to_message(e, &inbox.address))
            .collect())
    }
}

fn to_message(e: &Value, address: &str) -> Message {
    let date = parse_date(&e["date"]);
    let from = str_field(e, "from");
    let subject = str_field(e, "subject");
    let id = match e.get("_id").or_else(|| e.get("id")).and_then(Value::as_str) {
        Some(id) => id.to_string(),
        None => synthetic_id(&[&e["date"].to_string(), from, subject]),
    };
    Message {
        id,
        from: from.to_string(),
        to: vec![match str_field(e, "to") {
            "" => address.to_string(),
            to => to.to_string(),
        }],
        subject: subject.to_string(),
        date,
        text: opt_str(e, "body"),
        html: opt_str(e, "html"),
        attachments: Vec::new(),
    }
}

impl Provider for TempmailLol {
    fn id(&self) -> &'static str {
        "tempmail_lol"
    }
    fn name(&self) -> &'static str {
        "tempmail.lol"
    }
    fn website(&self) -> &'static str {
        "https://tempmail.lol"
    }

    fn caps(&self) -> Capabilities {
        Capabilities {
            private: true,
            custom_user: false,
            choose_domain: false,
            attachments: false,
            raw: false,
            delete_message: false,
            delete_inbox: false,
            login: false,
            lifetime: "1 hour",
        }
    }

    fn domains(&self) -> Result<Vec<String>> {
        Err(unsupported(self.id(), "listing domains (they rotate)"))
    }

    fn health(&self) -> Result<()> {
        // An unknown token gets a JSON 404 `{"expired":true}`: the API is up.
        match self.http.json::<Value>(|| {
            self.http
                .client()
                .get(format!("{}/inbox", self.base))
                .query(&[("token", "tmpmail-health-check")])
        }) {
            Ok(_) | Err(Error::NotFound(_)) => Ok(()),
            Err(e) => Err(e),
        }
    }

    fn create(&self, opts: &CreateOpts) -> Result<Inbox> {
        if opts.user.is_some() {
            return Err(unsupported(self.id(), "choosing the username"));
        }
        if opts.domain.is_some() {
            return Err(unsupported(self.id(), "choosing the domain"));
        }
        let v: Value = self.http.json(|| {
            self.http
                .client()
                .post(format!("{}/inbox/create", self.base))
                .json(&json!({}))
        })?;
        let (address, token) = (str_field(&v, "address"), str_field(&v, "token"));
        if address.is_empty() || token.is_empty() {
            return Err(self.http.bad("address or token missing"));
        }
        let mut inbox = Inbox::new(self.id(), address.to_string(), json!({ "token": token }));
        inbox.expires_at = Some(Utc::now() + Duration::hours(1));
        Ok(inbox)
    }

    fn list(&self, inbox: &mut Inbox) -> Result<Vec<Summary>> {
        Ok(self.emails(inbox)?.iter().map(Message::summary).collect())
    }

    fn get(&self, inbox: &mut Inbox, id: &str) -> Result<Message> {
        self.emails(inbox)?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| message_not_found(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mockito::{Matcher, Server};

    #[test]
    fn create_and_list_with_synthetic_ids() {
        let mut s = Server::new();
        s.mock("POST", "/inbox/create")
            .with_body(r#"{"address":"a1@kf.imagesthere.com","token":"t1"}"#)
            .create();
        s.mock("GET", "/inbox")
            .match_query(Matcher::UrlEncoded("token".into(), "t1".into()))
            .with_body(
                r#"{"emails":[{"from":"x@y.z","to":"a1@kf.imagesthere.com","subject":"S",
                "body":"code 9999","html":null,"date":1791194910000}],"expired":false}"#,
            )
            .create();
        let p = TempmailLol {
            base: s.url(),
            http: Http::new("tempmail_lol"),
        };
        let mut inbox = p.create(&CreateOpts::default()).unwrap();
        let list = p.list(&mut inbox).unwrap();
        assert_eq!(list.len(), 1);
        let msg = p.get(&mut inbox, &list[0].id).unwrap();
        assert_eq!(msg.text.as_deref(), Some("code 9999"));
        assert!(msg.html.is_none());
    }

    #[test]
    fn expired_inbox_is_reported() {
        let mut s = Server::new();
        s.mock("GET", "/inbox")
            .match_query(Matcher::Any)
            .with_body(r#"{"emails":[],"expired":true}"#)
            .create();
        let p = TempmailLol {
            base: s.url(),
            http: Http::new("tempmail_lol"),
        };
        let mut inbox = Inbox::new("tempmail_lol", "a@b.c".into(), json!({"token":"t"}));
        assert!(matches!(p.list(&mut inbox), Err(Error::Expired(_))));
    }
}
