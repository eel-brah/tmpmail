//! Turning untrusted mail content into something safe to print or open.

use std::borrow::Cow;
use std::collections::HashSet;

/// Removes control characters (ANSI escapes, carriage returns, bidi
/// overrides) so a message cannot rewrite the user's terminal. Keeps
/// newlines and tabs.
pub fn clean(s: &str) -> String {
    s.chars()
        .filter(|&c| c == '\n' || c == '\t' || !(c.is_control() || is_bidi_control(c)))
        .collect()
}

/// Like [`clean`] but also flattens newlines and tabs, for one-line fields.
pub fn clean_line(s: &str) -> String {
    let flat: String = clean(s)
        .chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .collect();
    flat.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// Plain-text rendering of an HTML body, without loading anything remote.
pub fn html_to_text(html: &str, width: usize) -> String {
    html2text::from_read(html.as_bytes(), width.max(20)).unwrap_or_else(|_| strip_tags(html))
}

fn strip_tags(html: &str) -> String {
    let re = regex::Regex::new(r"(?s)<[^>]*>").unwrap();
    re.replace_all(html, " ").into_owned()
}

/// Best text body for a message: the text part, else the HTML converted.
pub fn body_text(text: Option<&str>, html: Option<&str>, width: usize) -> String {
    match (text, html) {
        (Some(t), _) if !t.trim().is_empty() => t.to_string(),
        (_, Some(h)) => html_to_text(h, width),
        _ => String::new(),
    }
}

/// Sanitized standalone HTML document for viewing a message in a browser:
/// [`sanitize_mail_html`]'s body in a document whose meta CSP blocks scripts
/// and (unless `allow_remote_images`) remote images, with no referrer.
pub fn safe_html_document(subject: &str, html: &str, allow_remote_images: bool) -> String {
    let body = sanitize_mail_html(html, allow_remote_images, None);
    let img_src = if allow_remote_images {
        "data: cid: https: http:"
    } else {
        "data: cid:"
    };
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src {img_src}; style-src 'unsafe-inline'\">\n\
         <meta name=\"referrer\" content=\"no-referrer\">\n\
         <title>{}</title></head><body>\n{body}\n</body></html>\n",
        escape_html(subject)
    )
}

/// How the desktop app shows a mail body, where it differs from a browser
/// file: links open as popups (which the app hands to the system browser),
/// and `cid:` images point at the app's mail scheme.
pub struct Frame<'a> {
    /// Prefix for inline images: `cid:ID` becomes `{cid_base}{ID}` with the
    /// id percent-encoded.
    pub cid_base: &'a str,
}

/// Mail HTML with scripts, forms, frames and event handlers removed, links
/// marked noopener, only http/https/mailto/cid/data URLs, and remote images
/// (tracking pixels, also through `url(...)` in styles) dropped unless
/// `allow_remote_images`. `<style>` blocks are dropped too. With a `frame`,
/// links get `target="_blank"`, `cid:` images are rewritten, and plain
/// `http:` images are dropped even when remote images are allowed (the
/// frame's policy would block them anyway).
pub fn sanitize_mail_html(html: &str, allow_remote_images: bool, frame: Option<&Frame>) -> String {
    let mut builder = ammonia::Builder::default();
    builder
        .add_generic_attributes(["style", "align", "bgcolor", "width", "height"])
        .add_tag_attributes("td", ["valign", "colspan", "rowspan"])
        .add_tag_attributes("table", ["cellpadding", "cellspacing", "border"])
        .link_rel(Some("noopener noreferrer nofollow"))
        .url_schemes(HashSet::from(["http", "https", "mailto", "cid", "data"]));
    if frame.is_some() {
        // Image-map areas are links too; without a target they would
        // navigate the frame itself.
        builder
            .set_tag_attribute_value("a", "target", "_blank")
            .set_tag_attribute_value("area", "target", "_blank");
    }
    let cid_base = frame.map(|f| f.cid_base.to_string());
    builder.attribute_filter(move |element, attribute, value| {
        if element == "img" && attribute == "src" {
            if let (Some(base), Some(id)) = (&cid_base, value.strip_prefix("cid:")) {
                return Some(Cow::Owned(format!("{base}{}", percent_encode(id))));
            }
            let remote = !(value.starts_with("data:") || value.starts_with("cid:"));
            if remote && !allow_remote_images {
                return None;
            }
            // The app's frame loads remote images over https only.
            if cid_base.is_some() && is_http(value) {
                return None;
            }
        }
        // Inline styles can fetch remote resources through url(...). The
        // app's frame keeps the rest of the style (colors, spacing); the
        // CLI's file drops the whole attribute, as it always has.
        if attribute == "style" && !allow_remote_images {
            if cid_base.is_some() {
                return style_without_resources(value).map(Cow::Owned);
            }
            if value.to_ascii_lowercase().contains("url(") {
                return None;
            }
        }
        Some(Cow::Borrowed(value))
    });
    builder.clean(html).to_string()
}

/// A `style` attribute without the declarations that could load anything:
/// `url(...)`, image functions, and CSS escapes (which could spell `url(`
/// in a way a plain search misses). `None` when nothing is left.
fn style_without_resources(style: &str) -> Option<String> {
    let kept: Vec<&str> = declarations(style)
        .into_iter()
        .map(str::trim)
        .filter(|d| {
            let lower = d.to_ascii_lowercase();
            !d.is_empty()
                && !lower.contains('\\')
                && ![
                    "url(",
                    "image(",
                    "image-set(",
                    "src(",
                    "cross-fade(",
                    "element(",
                ]
                .iter()
                .any(|f| lower.contains(f))
        })
        .collect();
    (!kept.is_empty()).then(|| kept.join(";"))
}

/// Splits CSS declarations at the semicolons outside quotes and brackets,
/// so `url(data:image/png;base64,...)` stays one declaration.
fn declarations(style: &str) -> Vec<&str> {
    let (mut out, mut start, mut depth, mut quote) = (Vec::new(), 0, 0usize, None);
    let mut chars = style.char_indices();
    while let Some((i, c)) = chars.next() {
        match (quote, c) {
            (Some(_), '\\') => {
                chars.next();
            }
            (Some(q), _) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth = depth.saturating_sub(1),
            (None, ';') if depth == 0 => {
                out.push(&style[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&style[start..]);
    out
}

/// Whether sanitized mail HTML sets colors or backgrounds of its own
/// (`bgcolor`, or `color`/`background` in a `style`). The app shows such
/// mail inverted in dark mode, and mail without colors in the theme's own.
pub fn has_own_colors(sanitized: &str) -> bool {
    static STYLED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"(?i)\sbgcolor="|\sstyle="(?:[^"]*[;\s])?(?:background(?:-color)?|color)\s*:"#,
        )
        .unwrap()
    });
    STYLED.is_match(sanitized)
}

/// Whether a URL is plain `http:` as a browser would parse it: leading
/// spaces and control characters are ignored, and tabs and newlines
/// anywhere.
fn is_http(url: &str) -> bool {
    let scheme: String = url
        .trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .take(5)
        .collect();
    scheme.eq_ignore_ascii_case("http:")
}

/// Percent-encodes everything but unreserved URL characters.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Safe basename for saving an attachment: no path separators, no leading
/// dots, no control characters.
pub fn safe_filename(name: &str, fallback: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = clean_line(base)
        .chars()
        .map(|c| {
            if matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_start_matches('.').trim();
    if trimmed.is_empty() {
        fallback.to_string()
    } else {
        trimmed.chars().take(200).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_terminal_escapes() {
        let evil = "Hi\x1b[2J\x1b]0;pwned\x07 there\r\nnext\u{202E}line";
        assert_eq!(clean(evil), "Hi[2J]0;pwned there\nnextline");
        assert_eq!(clean_line("a\n\tb   c"), "a b c");
    }

    #[test]
    fn safe_html_drops_scripts_and_tracking_pixels() {
        let html = r#"<p onclick="x()">Hi<script>alert(1)</script></p>
            <img src="https://track.example/p.gif"><img src="cid:logo">
            <a href="https://example.com/verify">go</a><form><input></form>
            <div style="background:url(https://t.example/x)">bg</div>"#;
        let doc = safe_html_document("S <x>", html, false);
        assert!(!doc.contains("<script"));
        assert!(!doc.contains("onclick"));
        assert!(!doc.contains("track.example"));
        assert!(!doc.contains("t.example"));
        assert!(doc.contains("cid:logo"));
        assert!(doc.contains("noopener"));
        assert!(!doc.contains("<form"));
        assert!(doc.contains("<title>S &lt;x&gt;</title>"));
        assert!(doc.contains("img-src data: cid:;"));

        let with_images = safe_html_document("S", html, true);
        assert!(with_images.contains("track.example"));
    }

    /// `read --html` output from before the sanitizer was split out of
    /// `safe_html_document`: it must not change by a byte.
    #[test]
    fn safe_html_document_is_unchanged() {
        let html = r#"<p onclick="x()" style="color:red">Hi<script>alert(1)</script></p>
            <style>p{color:blue}</style><img src="https://track.example/p.gif" alt="t">
            <img src="cid:logo@x"><a href="https://example.com/verify" target="_top">go</a>
            <a href="javascript:alert(1)">js</a><form><input></form>
            <table cellpadding="2"><tr><td valign="top" colspan="2">c</td></tr></table>
            <div style="background:url(https://t.example/x)">bg</div>"#;
        let expected = "<!doctype html>\n<html><head><meta charset=\"utf-8\">\n<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data: cid:; style-src 'unsafe-inline'\">\n<meta name=\"referrer\" content=\"no-referrer\">\n<title>S &lt;x&gt;</title></head><body>\n<p style=\"color:red\">Hi</p>\n            <img alt=\"t\">\n            <img src=\"cid:logo@x\"><a href=\"https://example.com/verify\" rel=\"noopener noreferrer nofollow\">go</a>\n            <a rel=\"noopener noreferrer nofollow\">js</a>\n            <table cellpadding=\"2\"><tbody><tr><td valign=\"top\" colspan=\"2\">c</td></tr></tbody></table>\n            <div>bg</div>\n</body></html>\n";
        assert_eq!(safe_html_document("S <x>", html, false), expected);
        let with_images = "<!doctype html>\n<html><head><meta charset=\"utf-8\">\n<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data: cid: https: http:; style-src 'unsafe-inline'\">\n<meta name=\"referrer\" content=\"no-referrer\">\n<title>S &lt;x&gt;</title></head><body>\n<p style=\"color:red\">Hi</p>\n            <img src=\"https://track.example/p.gif\" alt=\"t\">\n            <img src=\"cid:logo@x\"><a href=\"https://example.com/verify\" rel=\"noopener noreferrer nofollow\">go</a>\n            <a rel=\"noopener noreferrer nofollow\">js</a>\n            <table cellpadding=\"2\"><tbody><tr><td valign=\"top\" colspan=\"2\">c</td></tr></tbody></table>\n            <div style=\"background:url(https://t.example/x)\">bg</div>\n</body></html>\n";
        assert_eq!(safe_html_document("S <x>", html, true), with_images);
    }

    #[test]
    fn frame_mode_opens_links_in_popups_and_serves_cid_images() {
        let html = r#"<a href="https://example.com/">go</a><img src="cid:logo@x y">
            <map name="m"><area href="https://example.com/a" shape="rect"></map>"#;
        let frame = Frame {
            cid_base: "tmpmail-mail://localhost/m1/cid/",
        };
        let body = sanitize_mail_html(html, false, Some(&frame));
        assert_eq!(body.matches(r#"target="_blank""#).count(), 2, "{body}");
        assert!(
            body.contains(r#"src="tmpmail-mail://localhost/m1/cid/logo%40x%20y""#),
            "{body}"
        );
        assert!(!sanitize_mail_html(html, false, None).contains("target"));
    }

    #[test]
    fn frame_mode_loads_remote_images_over_https_only() {
        let html = "<img src=\"https://a.example/x.png\"><img src=\"HTTP://b.example/y.png\">\
            <img src=\" ht&#9;tp://b.example/z.png\"><img src=\"\nhttp://b.example/w.png\">";
        let frame = Frame { cid_base: "x:" };
        let on = sanitize_mail_html(html, true, Some(&frame));
        assert!(on.contains("https://a.example/x.png"), "{on}");
        assert!(!on.contains("b.example"), "{on}");
        let off = sanitize_mail_html(html, false, Some(&frame));
        assert!(!off.contains("example"), "{off}");
        // The CLI's file keeps both; its policy allows http too.
        assert!(sanitize_mail_html(html, true, None).contains("b.example"));
    }

    #[test]
    fn frame_mode_strips_only_style_resources() {
        let frame = Frame { cid_base: "x:" };
        let html = r#"<p style="color:red; background:url(https://t.example/x) ; padding:4px">a</p>
            <p style="background-image:url('data:image/png;base64,AA==');margin:0">b</p>
            <p style="b\61ckground:u\72l(https://t.example/y);color:blue">c</p>
            <p style="background:-webkit-image-set('https://t.example/z' 1x)">d</p>"#;
        let off = sanitize_mail_html(html, false, Some(&frame));
        assert!(!off.contains("example"), "{off}");
        assert!(!off.contains("base64"), "{off}");
        assert!(off.contains(r#"style="color:red;padding:4px""#), "{off}");
        assert!(off.contains(r#"style="margin:0""#), "{off}");
        assert!(off.contains(r#"style="color:blue""#), "{off}");
        assert!(off.contains("<p>d</p>"), "{off}");
        // With images on, styles stay as the sender wrote them.
        let on = sanitize_mail_html(html, true, Some(&frame));
        assert!(on.contains("url(https://t.example/x)"), "{on}");
    }

    #[test]
    fn own_colors_are_detected() {
        assert!(has_own_colors(r##"<td bgcolor="#fff">x</td>"##));
        assert!(has_own_colors(r##"<p style="margin:0; color:#333">x</p>"##));
        assert!(has_own_colors(
            r#"<div style="background-color:red">x</div>"#
        ));
        assert!(!has_own_colors(r#"<p style="border-color:red">x</p>"#));
        assert!(!has_own_colors("<p>color: red</p>"));
        assert!(!has_own_colors("<pre>plain</pre>"));
    }

    #[test]
    fn html_to_text_keeps_content() {
        let t = html_to_text("<p>Your code is <b>123456</b></p>", 80);
        assert!(t.contains("123456"), "{t}");
    }

    #[test]
    fn filenames_cannot_escape_the_target_dir() {
        assert_eq!(safe_filename("../../etc/passwd", "f"), "passwd");
        assert_eq!(safe_filename("..\\..\\boot.ini", "f"), "boot.ini");
        assert_eq!(safe_filename("...", "fallback"), "fallback");
        assert_eq!(safe_filename(".bashrc", "f"), "bashrc");
        assert_eq!(safe_filename("a:b?.txt", "f"), "a_b_.txt");
    }
}
