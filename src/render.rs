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
/// scripts, forms and frames removed, links marked noopener, and remote
/// images (tracking pixels) dropped unless `allow_remote_images`.
pub fn safe_html_document(subject: &str, html: &str, allow_remote_images: bool) -> String {
    let mut builder = ammonia::Builder::default();
    builder
        .add_generic_attributes(["style", "align", "bgcolor", "width", "height"])
        .add_tag_attributes("td", ["valign", "colspan", "rowspan"])
        .add_tag_attributes("table", ["cellpadding", "cellspacing", "border"])
        .link_rel(Some("noopener noreferrer nofollow"))
        .url_schemes(HashSet::from(["http", "https", "mailto", "cid", "data"]))
        .attribute_filter(move |element, attribute, value| {
            if element == "img" && attribute == "src" {
                let remote = !(value.starts_with("data:") || value.starts_with("cid:"));
                if remote && !allow_remote_images {
                    return None;
                }
            }
            // Inline styles can fetch remote resources through url(...).
            if attribute == "style"
                && value.to_ascii_lowercase().contains("url(")
                && !allow_remote_images
            {
                return None;
            }
            Some(Cow::Borrowed(value))
        });
    let body = builder.clean(html).to_string();
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
