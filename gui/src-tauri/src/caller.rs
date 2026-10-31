//! Who may call the app's commands: only the app's own page in the main
//! window. Every command call passes [`check`] first (the invoke handler in
//! `main.rs` wraps the generated one), so no command can skip it.
//!
//! Why this is needed: on WebView2, Tauri injects its bridge
//! (`__TAURI_INTERNALS__`, with the invoke key) into every frame, and the
//! mail scheme's origin counts as app-local for the capability. In CI
//! (2026-10-31, WebView2 155) an unsandboxed control frame on the mail
//! origin called `init` and it ran. Without this check, a script in such a
//! frame could call any command.
//!
//! What the check trusts, and why a frame can't fake it:
//!
//! - The `Origin` header of the call. Tauri has two ways in. The first is
//!   its IPC custom protocol (a `fetch` to `ipc://localhost`, or
//!   `http://ipc.localhost` on Windows), and there the engine sets `Origin`
//!   to the origin of the frame that made the request. `Origin` is a
//!   forbidden request header: a script can't set or change it. A frame on
//!   the mail scheme sends `tmpmail-mail://localhost`
//!   (`http://tmpmail-mail.localhost`), a sandboxed frame `null`.
//! - The second way is a `postMessage` fallback, whose "headers" are
//!   whatever the script writes, so a forged `Origin` would pass there. The
//!   probe showed that on WebKitGTK any frame can post to it (a frame that
//!   knows the invoke key got a forged call accepted). So
//!   [`close_post_message`] removes wry's `ipc` message handler on WebKitGTK
//!   and WKWebView. The app's page uses the protocol anyway; the cost is
//!   that if one of its `fetch`es ever fails, Tauri's JS switches to the
//!   dead route for good and calls hang until the window reloads.
//! - The main webview's top-level URL, read from the engine (never from the
//!   call): it must be the app's page. This is what covers WebView2's
//!   `postMessage`: only the top-level document can post there (wry listens
//!   to `CoreWebView2.WebMessageReceived`, which frames don't reach). Web
//!   messages are also turned off there, but WebView2 applies that setting
//!   from the next document on, and the app's page may already be loading.
//!
//! Plugin commands don't pass through the app's invoke handler. The page
//! may use only `core:event:allow-listen` (no unlisten, which would let a
//! frame with the bridge remove the app's listeners: their ids are
//! sequential). Events are delivered by evaluating script in the top-level
//! document, so a frame that listens gets nothing.

use tauri::ipc::InvokeMessage;
use tauri::{Runtime, Url, WebviewWindow};

/// The error a refused call gets.
pub const REFUSED: &str = "tmpmail refused the call: it does not come from the app's page";

/// The label of the only webview that may call commands.
const MAIN: &str = "main";

/// Whether `origin` (an `Origin` header value) is the app page's own:
/// `tauri://localhost` (Linux, macOS), `http(s)://tauri.localhost`
/// (Windows), and the Vite dev server in debug builds.
pub fn app_origin(origin: &str) -> bool {
    matches!(
        origin,
        "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
    ) || cfg!(debug_assertions) && origin == "http://localhost:1420"
}

/// Whether a URL is on the app page's origin. (`Url::origin` is opaque for
/// custom schemes, so it's rebuilt from the parts.)
pub fn app_page(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return false;
    };
    let origin = match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    };
    app_origin(&origin) && url.username().is_empty() && url.password().is_none()
}

/// What the engine reports about a call's sender.
#[derive(Debug, Default)]
pub struct Caller {
    /// The webview's label.
    pub label: String,
    /// The webview's top-level URL.
    pub top: Option<Url>,
    /// The call's `Origin` header, if it has exactly one that is text.
    pub origin: Option<String>,
}

impl Caller {
    pub fn of<R: Runtime>(message: &InvokeMessage<R>) -> Caller {
        let webview = message.webview_ref();
        let mut values = message.headers().get_all("origin").iter();
        let origin = match (values.next(), values.next()) {
            (Some(v), None) => v.to_str().ok().map(str::to_string),
            _ => None,
        };
        Caller {
            label: webview.label().to_string(),
            top: webview.url().ok(),
            origin,
        }
    }

    /// The decision; see the module docs.
    pub fn allowed(&self) -> bool {
        self.label == MAIN
            && self.top.as_ref().is_some_and(app_page)
            && self.origin.as_deref().is_some_and(app_origin)
    }
}

/// Accepts a call only from the app's page. Returns what it looked at, so
/// the probe can record exactly that.
pub fn check<R: Runtime>(message: &InvokeMessage<R>) -> (Caller, Result<(), String>) {
    let caller = Caller::of(message);
    let verdict = if caller.allowed() {
        Ok(())
    } else {
        Err(REFUSED.to_string())
    };
    (caller, verdict)
}

/// Turns off Tauri's `postMessage` IPC route in the window's webview, so
/// every call arrives over the IPC protocol with an `Origin` the engine
/// set. Called right after the window is built (see the module docs for
/// what that means on WebView2).
pub fn close_post_message(window: &WebviewWindow) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    window
        .with_webview(move |webview| {
            let _ = tx.send(close(&webview));
        })
        .map_err(|e| e.to_string())?;
    rx.recv()
        .map_err(|_| "the webview didn't answer".to_string())?
}

/// The name wry registers its message handler under.
#[cfg(any(target_os = "linux", target_os = "macos"))]
const HANDLER: &str = "ipc";

#[cfg(target_os = "linux")]
fn close(webview: &tauri::webview::PlatformWebview) -> Result<(), String> {
    use webkit2gtk::{UserContentManagerExt, WebViewExt};
    let manager = webview
        .inner()
        .user_content_manager()
        .ok_or("the webview has no user content manager")?;
    manager.unregister_script_message_handler(HANDLER);
    Ok(())
}

#[cfg(target_os = "macos")]
fn close(webview: &tauri::webview::PlatformWebview) -> Result<(), String> {
    use objc2_foundation::NSString;
    use objc2_web_kit::WKUserContentController;
    let controller = webview.controller().cast::<WKUserContentController>();
    if controller.is_null() {
        return Err("the webview has no user content controller".into());
    }
    // SAFETY: Tauri hands out the webview's live WKUserContentController,
    // and this runs on the main thread (`with_webview`).
    unsafe {
        (*controller).removeScriptMessageHandlerForName(&NSString::from_str(HANDLER));
    }
    Ok(())
}

#[cfg(windows)]
fn close(webview: &tauri::webview::PlatformWebview) -> Result<(), String> {
    // SAFETY: plain COM calls on the webview's own controller, on the main
    // thread (`with_webview`).
    unsafe {
        webview
            .controller()
            .CoreWebView2()
            .and_then(|core| core.Settings())
            .and_then(|settings| settings.SetIsWebMessageEnabled(false))
            .map_err(|e| e.to_string())
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn close(_webview: &tauri::webview::PlatformWebview) -> Result<(), String> {
    Err("no way to close the postMessage route on this system".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn only_the_apps_origins() {
        assert!(app_origin("tauri://localhost"));
        assert!(app_origin("http://tauri.localhost"));
        assert!(app_origin("https://tauri.localhost"));
        assert_eq!(app_origin("http://localhost:1420"), cfg!(debug_assertions));
        for bad in [
            "tmpmail-mail://localhost",
            "http://tmpmail-mail.localhost",
            "null",
            "",
            "tauri://localhost/",
            "tauri://localhost.evil",
            "http://tauri.localhost:8080",
            "http://localhost:1421",
            "https://example.com",
            "TAURI://LOCALHOST",
        ] {
            assert!(!app_origin(bad), "{bad}");
        }
    }

    #[test]
    fn the_top_level_must_be_the_apps_page() {
        assert!(app_page(&url("tauri://localhost/")));
        assert!(app_page(&url("tauri://localhost/index.html")));
        assert!(app_page(&url("http://tauri.localhost/index.html")));
        assert!(!app_page(&url("tmpmail-mail://localhost/mail/1")));
        assert!(!app_page(&url("http://tmpmail-mail.localhost/mail/1")));
        assert!(!app_page(&url("http://user@tauri.localhost/")));
        assert!(!app_page(&url("https://example.com/")));
        assert!(!app_page(&url("about:blank")));
    }

    fn allowed(label: &str, top: Option<&Url>, origin: Option<&str>) -> bool {
        Caller {
            label: label.into(),
            top: top.cloned(),
            origin: origin.map(str::to_string),
        }
        .allowed()
    }

    #[test]
    fn every_part_must_hold() {
        let page = url("tauri://localhost/index.html");
        let mail = url("tmpmail-mail://localhost/mail/1");
        assert!(allowed("main", Some(&page), Some("tauri://localhost")));
        // A frame on the mail origin, on an engine that gives frames the bridge.
        assert!(!allowed(
            "main",
            Some(&page),
            Some("tmpmail-mail://localhost")
        ));
        assert!(!allowed(
            "main",
            Some(&page),
            Some("http://tmpmail-mail.localhost")
        ));
        // A sandboxed frame, and a call with no Origin at all.
        assert!(!allowed("main", Some(&page), Some("null")));
        assert!(!allowed("main", Some(&page), None));
        // The top-level document isn't the app (a forged header can't help).
        assert!(!allowed("main", Some(&mail), Some("tauri://localhost")));
        assert!(!allowed("main", None, Some("tauri://localhost")));
        // Another webview.
        assert!(!allowed("other", Some(&page), Some("tauri://localhost")));
    }
}
