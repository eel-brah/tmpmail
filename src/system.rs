//! Talking to the desktop: clipboard, browser, notifications.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime};

use anyhow::{Context, anyhow, bail};

fn on_path(bin: &str) -> bool {
    // Windows runs `less` as `less.exe` (or another `PATHEXT` extension).
    let exts: Vec<String> = if cfg!(windows) {
        path_exts(std::env::var("PATHEXT").ok().as_deref())
    } else {
        Vec::new()
    };
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| in_dir(&dir, bin, &exts)))
}

/// The extensions to try after the bare name, from `PATHEXT`
/// (default `.COM;.EXE;.BAT;.CMD`).
fn path_exts(pathext: Option<&str>) -> Vec<String> {
    pathext
        .filter(|p| !p.trim().is_empty())
        .unwrap_or(".COM;.EXE;.BAT;.CMD")
        .split(';')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_string)
        .collect()
}

/// Whether `dir` holds `bin` as is or with one of `exts` appended.
fn in_dir(dir: &Path, bin: &str, exts: &[String]) -> bool {
    dir.join(bin).is_file()
        || exts
            .iter()
            .any(|ext| dir.join(format!("{bin}{ext}")).is_file())
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
        .or_else(|| std::env::var("BROWSER").ok().filter(|b| !b.is_empty()));
    #[cfg(windows)]
    if cmd.is_none() {
        return windows_open(target);
    }
    let cmd = cmd
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

/// Explorer hands the target to the shell's default handler, so it opens
/// URLs in the default browser and files with their default app. Unlike
/// `cmd /C start`, no shell parses the target, so `&`, `|`, `^` and `%`
/// in a mail's link are passed through as is.
#[cfg(windows)]
fn windows_open(target: &str) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    Command::new("explorer.exe")
        .raw_arg(explorer_arg(target)?)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("cannot run explorer.exe")?;
    // Explorer exits with 1 even when it worked, so the status means nothing.
    Ok(())
}

/// Explorer's argument, always quoted so commas and spaces stay part of the
/// target. A quote can't be escaped there; URLs encode it as `%22` and
/// Windows paths can't contain one, so it is refused.
#[cfg(any(windows, test))]
fn explorer_arg(target: &str) -> anyhow::Result<String> {
    if target.contains('"') || target.chars().any(char::is_control) {
        bail!("cannot open {target:?}: it contains a quote or control character");
    }
    Ok(format!("\"{target}\""))
}

/// Columns and rows of the terminal on stdout, else `$COLUMNS` and `$LINES`
/// (rows 0 when unknown).
pub fn term_size() -> Option<(usize, usize)> {
    use std::io::IsTerminal;
    // crossterm asks the terminal itself (/dev/tty, or the Windows console),
    // so only trust it when stdout is that terminal.
    if std::io::stdout().is_terminal() {
        if let Ok((cols, rows)) = ratatui::crossterm::terminal::size() {
            if cols > 0 {
                return Some((cols as usize, rows as usize));
            }
        }
    }
    let env = |key: &str| std::env::var(key).ok()?.parse::<usize>().ok();
    Some((env("COLUMNS")?, env("LINES").unwrap_or(0)))
}

/// Pipes `text` into a pager: `pager`, else `$PAGER`, else `less`. Like git,
/// sets `LESS=FRX` when unset so colors work and short text doesn't page.
/// Windows rarely has `less`, so there the text is printed when it's missing.
pub fn page(text: &str, pager: Option<&str>) -> anyhow::Result<()> {
    let cmd = pager
        .map(str::to_string)
        .or_else(|| std::env::var("PAGER").ok())
        .filter(|p| !p.trim().is_empty());
    if cmd.is_none() && cfg!(windows) && !on_path("less") {
        print!("{text}");
        return Ok(());
    }
    let cmd = cmd.unwrap_or_else(|| "less".into());
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
    if on_path("notify-send") {
        let _ = Command::new("notify-send")
            .args(["--app-name=tmpmail", "--", title, body])
            .status();
    } else if on_path("osascript") {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let _ = Command::new("osascript")
            .arg("-e")
            .arg(format!(
                "display notification \"{}\" with title \"{}\"",
                esc(body),
                esc(title)
            ))
            .status();
    } else {
        // Don't wait: PowerShell takes a second or so to start.
        #[cfg(windows)]
        let _ = windows_notify_command(title, body)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

/// A toast through Windows PowerShell's WinRT bridge, shown under
/// PowerShell's app id since tmpmail has none registered. Mail subjects are
/// attacker-controlled, so the title and body travel in environment
/// variables and become XML text nodes: the script itself never changes.
#[cfg(any(windows, test))]
fn windows_notify_command(title: &str, body: &str) -> Command {
    const SCRIPT: &str = "$ErrorActionPreference = 'Stop'; \
        $null = [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime]; \
        $m = [Windows.UI.Notifications.ToastNotificationManager]; \
        $xml = $m::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
        $text = $xml.GetElementsByTagName('text'); \
        $null = $text.Item(0).AppendChild($xml.CreateTextNode($env:TMPMAIL_NOTIFY_TITLE)); \
        $null = $text.Item(1).AppendChild($xml.CreateTextNode($env:TMPMAIL_NOTIFY_BODY)); \
        $app = '{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe'; \
        $m::CreateToastNotifier($app).Show([Windows.UI.Notifications.ToastNotification]::new($xml))";
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("TMPMAIL_NOTIFY_TITLE", title)
        .env("TMPMAIL_NOTIFY_BODY", body)
        .stdin(Stdio::null());
    cmd
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
    fn pathext_lookup_finds_tools_without_their_extension() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("less.EXE"), "").unwrap();
        std::fs::write(dir.path().join("clip.exe"), "").unwrap();
        let exts = path_exts(Some(".COM;.EXE; ;.BAT"));
        assert_eq!(exts, [".COM", ".EXE", ".BAT"]);
        assert!(in_dir(dir.path(), "less", &exts));
        assert!(in_dir(dir.path(), "clip.exe", &exts));
        assert!(!in_dir(dir.path(), "less", &[]));
        assert!(!in_dir(dir.path(), "more", &exts));
        assert_eq!(path_exts(None), [".COM", ".EXE", ".BAT", ".CMD"]);
        assert_eq!(path_exts(Some("")), path_exts(None));
    }

    #[test]
    fn explorer_arg_is_quoted_and_refuses_quotes() {
        let url = "https://x.com/v?a=1&b=%PATH%,2^|3";
        assert_eq!(explorer_arg(url).unwrap(), format!("\"{url}\""));
        let path = r"C:\Users\Jo Smith\AppData\Local\tmpmail\view\a.html";
        assert_eq!(explorer_arg(path).unwrap(), format!("\"{path}\""));
        assert!(explorer_arg("https://x.com/\" & calc").is_err());
        assert!(explorer_arg("https://x.com/\n").is_err());
    }

    #[test]
    fn windows_notify_keeps_mail_text_out_of_the_script() {
        let title = "tmpmail: a@x.com";
        let body = "'; Start-Process calc; $(calc) \"`";
        let cmd = windows_notify_command(title, body);
        assert!(
            cmd.get_args()
                .all(|a| !a.to_string_lossy().contains("calc"))
        );
        let envs: Vec<_> = cmd.get_envs().collect();
        assert!(envs.contains(&("TMPMAIL_NOTIFY_TITLE".as_ref(), Some(title.as_ref()))));
        assert!(envs.contains(&("TMPMAIL_NOTIFY_BODY".as_ref(), Some(body.as_ref()))));
    }

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
