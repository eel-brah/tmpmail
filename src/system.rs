//! Talking to the desktop: clipboard, browser, notifications.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use anyhow::{Context, anyhow, bail};

fn on_path(bin: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(bin).is_file()))
}

fn split_cmd(cmd: &str) -> anyhow::Result<(String, Vec<String>)> {
    let mut parts = cmd.split_whitespace().map(str::to_string);
    let prog = parts.next().ok_or_else(|| anyhow!("empty command"))?;
    Ok((prog, parts.collect()))
}

/// Picks a clipboard command for this session: Wayland, X11, macOS, WSL.
pub fn clipboard_command(custom: Option<&str>) -> anyhow::Result<String> {
    if let Some(cmd) = custom {
        return Ok(cmd.to_string());
    }
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    let x11 = std::env::var_os("DISPLAY").is_some();
    let candidates: &[(&str, &str, bool)] = &[
        ("wl-copy", "wl-copy", wayland),
        ("xclip", "xclip -selection clipboard", x11),
        ("xsel", "xsel --clipboard --input", x11),
        ("pbcopy", "pbcopy", true),
        ("clip.exe", "clip.exe", true),
        ("termux-clipboard-set", "termux-clipboard-set", true),
    ];
    candidates
        .iter()
        .find(|(bin, _, usable)| *usable && on_path(bin))
        .map(|(_, cmd, _)| cmd.to_string())
        .ok_or_else(|| {
            anyhow!("no clipboard tool found; install wl-clipboard, xclip or xsel, or pass --clipboard-cmd")
        })
}

pub fn copy(text: &str, custom: Option<&str>) -> anyhow::Result<()> {
    let cmd = clipboard_command(custom)?;
    let (prog, args) = split_cmd(&cmd)?;
    let mut child = Command::new(&prog)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("cannot run {prog}"))?;
    child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(text.as_bytes())?;
    // wl-copy and xclip fork to serve the selection; the parent exits quickly.
    let status = child.wait()?;
    if !status.success() {
        bail!("{prog} exited with {status}");
    }
    Ok(())
}

/// Opens a file or URL with the configured browser, `$BROWSER`, or the
/// platform opener.
pub fn open(target: &str, browser: Option<&str>) -> anyhow::Result<()> {
    let cmd = browser
        .map(str::to_string)
        .or_else(|| std::env::var("BROWSER").ok().filter(|b| !b.is_empty()))
        .or_else(|| {
            ["xdg-open", "open", "wslview"]
                .into_iter()
                .find(|b| on_path(b))
                .map(str::to_string)
        })
        .ok_or_else(|| anyhow!("no browser found; set $BROWSER or pass --browser"))?;
    let (prog, args) = split_cmd(&cmd)?;
    // Text browsers (w3m, lynx) need the terminal, so wait for them.
    let interactive = matches!(
        prog.as_str(),
        "w3m" | "lynx" | "links" | "elinks" | "browsh"
    );
    let mut command = Command::new(&prog);
    command.args(&args).arg(target);
    if interactive {
        command
            .status()
            .with_context(|| format!("cannot run {prog}"))?;
    } else {
        command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("cannot run {prog}"))?;
    }
    Ok(())
}

/// Columns and rows of the terminal on stdout, else `$COLUMNS` and `$LINES`
/// (rows 0 when unknown).
pub fn term_size() -> Option<(usize, usize)> {
    #[cfg(unix)]
    {
        // SAFETY: winsize is plain data; TIOCGWINSZ only writes into it, and
        // fails (leaving it zeroed) when stdout is not a terminal.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(libc::STDOUT_FILENO, libc::TIOCGWINSZ, &mut ws) } == 0
            && ws.ws_col > 0
        {
            return Some((ws.ws_col as usize, ws.ws_row as usize));
        }
    }
    let env = |key: &str| std::env::var(key).ok()?.parse::<usize>().ok();
    Some((env("COLUMNS")?, env("LINES").unwrap_or(0)))
}

/// Pipes `text` into a pager: `pager`, else `$PAGER`, else `less`. Like git,
/// sets `LESS=FRX` when unset so colors work and short text doesn't page.
pub fn page(text: &str, pager: Option<&str>) -> anyhow::Result<()> {
    let cmd = pager
        .map(str::to_string)
        .or_else(|| std::env::var("PAGER").ok())
        .filter(|p| !p.trim().is_empty())
        .unwrap_or_else(|| "less".into());
    let (prog, args) = split_cmd(&cmd)?;
    let mut command = Command::new(&prog);
    command.args(&args).stdin(Stdio::piped());
    if std::env::var_os("LESS").is_none() {
        command.env("LESS", "FRX");
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("cannot run {prog}"))?;
    let written = child
        .stdin
        .take()
        .expect("stdin is piped")
        .write_all(text.as_bytes());
    child.wait()?;
    match written {
        // Quitting the pager before the end closes the pipe; that's fine.
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
        _ => Ok(()),
    }
}

pub fn notify(title: &str, body: &str) {
    let _ = if on_path("notify-send") {
        Command::new("notify-send")
            .args(["--app-name=tmpmail", "--", title, body])
            .status()
    } else if on_path("osascript") {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "display notification \"{}\" with title \"{}\"",
                esc(body),
                esc(title)
            ))
            .status()
    } else {
        return;
    };
}

/// How long a copy written for the browser is kept. The browser reads it
/// right away, but may reload it, so it isn't deleted at once.
const VIEW_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

/// Writes a viewer file readable only by the user and returns its path.
/// Copies left over from earlier views are deleted once they are a day old.
pub fn write_view_file(
    dir: &Path,
    name: &str,
    contents: &str,
) -> crate::error::Result<std::path::PathBuf> {
    let views = dir.join("view");
    crate::store::create_private_dir(&views)?;
    prune_views(&views, VIEW_MAX_AGE);
    let path = views.join(name);
    crate::store::write_private(&path, contents.as_bytes())?;
    Ok(path)
}

/// Best effort: a file that can't be inspected or removed is left alone.
fn prune_views(views: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(views) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .ok()
            .filter(|m| m.is_file())
            .and_then(|m| m.modified().ok())
            .and_then(|t| now.duration_since(t).ok())
            .is_some_and(|age| age > max_age);
        if old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_view_files_are_pruned() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("view").join("old.html");
        let first = write_view_file(dir.path(), "old.html", "x").unwrap();
        assert_eq!(first, old);
        let day_ago = SystemTime::now() - VIEW_MAX_AGE - Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(day_ago)
            .unwrap();
        let new = write_view_file(dir.path(), "new.html", "y").unwrap();
        assert!(!old.exists());
        assert!(new.exists());
    }
}
