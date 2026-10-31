//! The `tmpmail-mail` URI scheme: serves opened messages, sanitized by
//! ammonia, to the preview's sandboxed iframe. Each response carries its own
//! Content-Security-Policy, so the mail frame's rules don't depend on the
//! app's, and "load images" is a different response, not a looser shell.
//!
//! Paths: `/mail/<key>[?images=1]` is a message body, `/mail/<key>/cid/<id>`
//! one of its inline images. Nothing here does network work: inline images
//! that aren't cached are fetched by the worker.

use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, UriSchemeResponder};
use tmpmail::provider::{Attachment, Message};
use tmpmail::render::{Frame, escape_html, sanitize_mail_html};

use crate::Shared;
use crate::worker::Req;

pub const SCHEME: &str = "tmpmail-mail";

/// The scheme's origin as the webview sees it: Windows (WebView2) maps
/// custom schemes to `http://<scheme>.localhost`.
pub fn origin() -> &'static str {
    if cfg!(windows) {
        "http://tmpmail-mail.localhost"
    } else {
        "tmpmail-mail://localhost"
    }
}

/// The policy of a mail document: no scripts, fonts, frames, forms or
/// connections; images only inline, from this scheme, and (once the user
/// asks) from https.
pub fn mail_csp(images: bool) -> String {
    let own = if cfg!(windows) {
        "http://tmpmail-mail.localhost"
    } else {
        "tmpmail-mail:"
    };
    let remote = if images { " https:" } else { "" };
    format!(
        "default-src 'none'; img-src data: {own}{remote}; style-src 'unsafe-inline'; \
         base-uri 'none'; form-action 'none'"
    )
}

/// Mail is shown on a white page whatever the app's theme: senders design
/// for one, and recoloring their HTML is guesswork.
const FRAME_CSS: &str = "html{background:#fff;color:#1a1a1a;color-scheme:light}\
body{margin:20px 24px;font:14px/1.55 system-ui,-apple-system,'Segoe UI',sans-serif;\
overflow-wrap:anywhere}img{max-width:100%;height:auto}\
pre{white-space:pre-wrap;font:13px/1.5 ui-monospace,monospace}";

pub fn handle(
    _app: &AppHandle,
    shared: &Shared,
    req: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let uri = req.uri();
    let path = uri.path().to_string();
    let query = uri.query().unwrap_or_default().to_string();
    #[cfg(debug_assertions)]
    if let Some(probe) = &shared.probe {
        probe.record_request(&path, &query);
    }
    if std::env::var_os("TMPMAIL_GUI_LOG").is_some() {
        eprintln!("{SCHEME}: {} {uri}", req.method());
    }
    if req.method() != tauri::http::Method::GET {
        return responder.respond(status(StatusCode::METHOD_NOT_ALLOWED));
    }
    let segments: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    match segments.as_slice() {
        ["mail", key] => {
            let images = query.split('&').any(|kv| kv == "images=1");
            let found = key.parse().ok().and_then(|key| {
                let mails = shared.mails.lock().expect("mails lock");
                mails.get(key).map(|m| (key, m.msg.clone()))
            });
            let Some((key, msg)) = found else {
                return responder.respond(not_found());
            };
            // Sanitizing a big mail takes a while: not on the webview's
            // thread.
            tauri::async_runtime::spawn_blocking(move || {
                let doc = mail_document(&msg, key, images);
                responder.respond(document(&doc, &mail_csp(images)));
            });
        }
        ["mail", key, "cid", cid] => {
            let (Ok(key), Some(cid)) = (key.parse::<u64>(), percent_decode(cid)) else {
                return responder.respond(not_found());
            };
            let cached = {
                let mails = shared.mails.lock().expect("mails lock");
                match mails.get(key) {
                    None => return responder.respond(not_found()),
                    Some(m) => m.inline.get(&cid).cloned(),
                }
            };
            match (cached, shared.worker.get()) {
                (Some((mime, bytes)), _) => responder.respond(image(&mime, bytes)),
                (None, Some(tx)) => {
                    // The worker answers (it may need the provider).
                    let _ = tx.send(Req::Inline {
                        key,
                        cid,
                        responder,
                    });
                }
                (None, None) => responder.respond(not_found()),
            }
        }
        #[cfg(debug_assertions)]
        ["probe", rest @ ..] if shared.probe.is_some() => {
            let probe = shared.probe.as_ref().expect("checked");
            responder.respond(probe.serve(rest));
        }
        _ => responder.respond(not_found()),
    }
}

/// The sanitized document for `msg`.
fn mail_document(msg: &Message, key: u64, images: bool) -> String {
    let text;
    let html = match (&msg.html, &msg.text) {
        (Some(h), _) => h.as_str(),
        (None, Some(t)) => {
            text = format!("<pre>{}</pre>", escape_html(t));
            text.as_str()
        }
        (None, None) => "",
    };
    let cid_base = format!("{}/mail/{key}/cid/", origin());
    let body = sanitize_mail_html(
        html,
        images,
        Some(&Frame {
            cid_base: &cid_base,
        }),
    );
    wrap(&body)
}

/// A full document around a sanitized body.
pub fn wrap(body: &str) -> String {
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\">\
         <meta name=\"referrer\" content=\"no-referrer\"><style>{FRAME_CSS}</style>\
         </head><body>\n{body}\n</body></html>\n"
    )
}

/// An HTML response with the given policy (`None`: no CSP header at all,
/// only for the probe's controls).
pub fn html_response(doc: &str, csp: Option<&str>) -> Response<Vec<u8>> {
    let mut b = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::REFERRER_POLICY, "no-referrer")
        .header(header::CACHE_CONTROL, "no-store");
    if let Some(csp) = csp {
        b = b.header(header::CONTENT_SECURITY_POLICY, csp);
    }
    b.body(doc.as_bytes().to_vec()).expect("valid response")
}

fn document(doc: &str, csp: &str) -> Response<Vec<u8>> {
    html_response(doc, Some(csp))
}

/// Inline images are served only as images, never as something a frame
/// could run.
pub fn image(content_type: &str, bytes: Vec<u8>) -> Response<Vec<u8>> {
    let ct = if content_type.starts_with("image/") && !content_type.contains("svg") {
        content_type
    } else {
        "application/octet-stream"
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, ct)
        .header(header::X_CONTENT_TYPE_OPTIONS, "nosniff")
        .header(header::CONTENT_SECURITY_POLICY, "default-src 'none'")
        .header(header::CACHE_CONTROL, "no-store")
        .body(bytes)
        .expect("valid response")
}

pub fn status(code: StatusCode) -> Response<Vec<u8>> {
    Response::builder()
        .status(code)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Vec::new())
        .expect("valid response")
}

pub fn not_found() -> Response<Vec<u8>> {
    status(StatusCode::NOT_FOUND)
}

/// The attachment a `cid:` reference most likely means, and the image type
/// to serve it as. Providers don't report Content-ID headers, so this
/// matches the attachment id or file name against the cid or its part
/// before `@`.
pub fn find_inline<'a>(msg: &'a Message, cid: &str) -> Option<(&'a Attachment, String)> {
    let short = cid.split('@').next().unwrap_or(cid);
    msg.attachments
        .iter()
        .filter(|a| [cid, short].iter().any(|c| a.id == *c || a.filename == *c))
        .find_map(|a| Some((a, inline_type(a)?)))
}

/// The raster image type an inline attachment is served as: its own, or
/// for `application/octet-stream` one known from its file extension. Never
/// SVG (it can carry script) and never anything that isn't an image.
fn inline_type(a: &Attachment) -> Option<String> {
    let ct = a.content_type.split(';').next().unwrap_or_default();
    let ct = ct.trim().to_ascii_lowercase();
    if ct.starts_with("image/") {
        return (!ct.contains("svg")).then_some(ct);
    }
    if ct != "application/octet-stream" {
        return None;
    }
    let ext = a.filename.rsplit_once('.')?.1.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        _ => return None,
    };
    Some(mime.to_string())
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cid_ids_round_trip() {
        assert_eq!(percent_decode("logo%40x%20y").as_deref(), Some("logo@x y"));
        assert_eq!(percent_decode("bad%4"), None);
    }

    fn attachment(filename: &str, content_type: &str) -> Attachment {
        Attachment {
            id: format!("id-{filename}"),
            filename: filename.into(),
            content_type: content_type.into(),
            size: None,
            url: None,
        }
    }

    #[test]
    fn inline_images_are_raster_images_only() {
        let msg = Message {
            attachments: vec![
                attachment("logo.png", "image/png; name=logo.png"),
                attachment("photo.JPG", "application/octet-stream"),
                attachment("x.svg", "image/svg+xml"),
                attachment("y.svg", "application/octet-stream"),
                attachment("page.html", "text/html"),
            ],
            ..Default::default()
        };
        let find = |cid: &str| find_inline(&msg, cid).map(|(a, t)| (a.filename.clone(), t));
        assert_eq!(
            find("logo.png@x"),
            Some(("logo.png".into(), "image/png".into()))
        );
        assert_eq!(
            find("id-photo.JPG"),
            Some(("photo.JPG".into(), "image/jpeg".into()))
        );
        assert_eq!(find("x.svg"), None);
        assert_eq!(find("y.svg"), None);
        assert_eq!(find("page.html"), None);
        assert_eq!(find("nothing"), None);
    }

    #[test]
    fn images_only_widen_img_src() {
        let off = mail_csp(false);
        let on = mail_csp(true);
        assert!(off.starts_with("default-src 'none';"));
        assert!(!off.contains("https:"));
        assert_eq!(on.replace(" https:", ""), off);
    }
}
