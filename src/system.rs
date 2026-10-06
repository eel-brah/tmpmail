//! Talking to the desktop: clipboard, browser, notifications.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

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

pub fn notify(title: &str, body: &str) {
    let _ = if on_path("notify-send") {
        Command::new("notify-send")
            .args(["--app-name=tmpmail", title, body])
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

/// Writes a viewer file readable only by the user and returns its path.
pub fn write_view_file(
    dir: &Path,
    name: &str,
    contents: &str,
) -> crate::error::Result<std::path::PathBuf> {
    let views = dir.join("view");
    crate::store::create_private_dir(&views)?;
    let path = views.join(name);
    crate::store::write_private(&path, contents.as_bytes())?;
    Ok(path)
}
