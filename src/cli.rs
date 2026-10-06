use clap::{Args, Parser, Subcommand};
use clap_complete::Shell;

const AFTER_HELP: &str = "\
Providers: mail.tm (default), DropMail.me, Guerrilla Mail, tempmail.lol, mail.gw,
and Maildrop (public inboxes, opt-in). `new` falls back to the next provider when
one is down. Thanks to mail.tm (https://mail.tm) for its free API.

Exit codes: 0 ok, 1 error, 2 wait timed out, 3 provider failure,
4 nothing found (no inbox, message or code), 64 usage error.";

#[derive(Debug, Parser)]
#[command(
    name = "tmpmail",
    version,
    about = "Temporary email from the terminal",
    flatten_help = true,
    after_help = AFTER_HELP
)]
pub struct Cli {
    #[command(flatten)]
    pub global: Global,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Args)]
pub struct Global {
    /// Machine-readable JSON output
    #[arg(long, global = true)]
    pub json: bool,

    /// Inbox to use (address, local part or label); defaults to the active one
    #[arg(long, short = 'i', global = true, env = "TMPMAIL_INBOX")]
    pub inbox: Option<String>,

    /// Disable colors (also honours NO_COLOR)
    #[arg(long, global = true)]
    pub no_color: bool,

    /// Only print essential output
    #[arg(long, short, global = true)]
    pub quiet: bool,

    /// Browser command for HTML and links (default: $BROWSER, xdg-open, open)
    #[arg(long, short = 'b', global = true)]
    pub browser: Option<String>,

    /// Clipboard command (default: auto-detect wl-copy, xclip, xsel, pbcopy)
    #[arg(long, global = true)]
    pub clipboard_cmd: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a new address and make it active
    #[command(visible_alias = "generate")]
    New(NewArgs),

    /// List messages in the inbox (or all inboxes with -a)
    #[command(visible_alias = "inbox")]
    Ls(LsArgs),

    /// Show a message (by id, list number, or --recent)
    #[command(visible_alias = "show")]
    Read(ReadArgs),

    /// Wait for a matching message to arrive
    Wait(WaitArgs),

    /// Extract a verification code (OTP) from a message
    Otp(OtpArgs),

    /// Extract links from a message, verification links first
    Links(LinksArgs),

    /// Stream new messages as they arrive
    Watch(WatchArgs),

    /// List a message's attachments
    Attachments(MessageRef),

    /// Download attachments
    Save(SaveArgs),

    /// Delete one message or all of them
    Delete(DeleteArgs),

    /// Print the active address
    Whoami,

    /// Copy the active address to the clipboard
    Copy,

    /// Show the active address as a QR code
    Qr,

    /// List available domains
    Domains(ProviderArg),

    /// List stored inboxes
    Inboxes(InboxesArgs),

    /// Switch the active inbox
    Use {
        /// Address, local part or label of a stored inbox
        address: String,
    },

    /// Attach an existing account (mail.tm / mail.gw, or a Maildrop name)
    Login(LoginArgs),

    /// Delete the inbox at the provider and forget it locally
    Drop(DropArgs),

    /// Replace the active address with a fresh one
    Rotate(RotateArgs),

    /// List providers, their capabilities and (with --check) their status
    Providers(ProvidersArgs),

    /// Print shell completions
    Completions { shell: Shell },
}

#[derive(Debug, Args)]
pub struct NewArgs {
    /// Username (local part); random and readable by default
    #[arg(long, short)]
    pub user: Option<String>,

    /// Domain, see `tmpmail domains`
    #[arg(long, short)]
    pub domain: Option<String>,

    /// Use only this provider (no fallback)
    #[arg(long, short)]
    pub provider: Option<String>,

    /// Label to find this inbox later (e.g. github)
    #[arg(long, short)]
    pub label: Option<String>,

    /// Account password (mail.tm / mail.gw); random by default
    #[arg(long, env = "TMPMAIL_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,

    /// Copy the new address to the clipboard
    #[arg(long, short)]
    pub copy: bool,

    /// Full address, e.g. otter42@maxxspace.com (sets user and domain)
    #[arg(conflicts_with_all = ["user", "domain"])]
    pub address: Option<String>,
}

#[derive(Debug, Args)]
pub struct MessageRef {
    /// Message id or its number in `tmpmail ls`; newest when omitted
    pub id: Option<String>,

    /// Use the newest message
    #[arg(long, short, conflicts_with = "id")]
    pub recent: bool,
}

#[derive(Debug, Args)]
pub struct ReadArgs {
    #[command(flatten)]
    pub msg: MessageRef,

    /// Open the sanitized HTML in a browser
    #[arg(long, conflicts_with = "raw")]
    pub html: bool,

    /// Load remote images in --html (off by default: they are tracking pixels)
    #[arg(long, requires = "html")]
    pub images: bool,

    /// Print the raw RFC 822 source
    #[arg(long)]
    pub raw: bool,

    /// Text only (the default; kept for tmpmail compatibility)
    #[arg(long, short, hide = true)]
    pub text: bool,
}

#[derive(Debug, Args)]
pub struct LsArgs {
    /// List every stored inbox, not just the active one
    #[arg(long, short = 'a')]
    pub all_inboxes: bool,
}

#[derive(Debug, Args, Clone, Default)]
pub struct WaitFilter {
    /// Watch every stored inbox; the first match in any of them wins
    #[arg(long, short = 'a')]
    pub all_inboxes: bool,

    /// Regex the sender must match (case-insensitive)
    #[arg(long)]
    pub from: Option<String>,

    /// Regex the subject must match (case-insensitive)
    #[arg(long)]
    pub subject: Option<String>,

    /// Regex the body must match (case-insensitive)
    #[arg(long)]
    pub body: Option<String>,

    /// Give up after this many seconds [default: 180]
    #[arg(long, short)]
    pub timeout: Option<u64>,

    /// Seconds between checks [default: 5]
    #[arg(long)]
    pub interval: Option<u64>,

    /// Also consider messages that were already there
    #[arg(long)]
    pub include_existing: bool,
}

#[derive(Debug, Args)]
pub struct WaitArgs {
    #[command(flatten)]
    pub filter: WaitFilter,

    /// Print only the verification code (waits for a message that has one)
    #[arg(long, conflicts_with = "link")]
    pub otp: bool,

    /// Print only the best link
    #[arg(long)]
    pub link: bool,

    /// Custom regex for --otp; first capture group is the code
    #[arg(long, requires = "otp")]
    pub otp_regex: Option<String>,

    /// Delete the message once it has been printed
    #[arg(long)]
    pub delete: bool,
}

#[derive(Debug, Args)]
pub struct OtpArgs {
    #[command(flatten)]
    pub msg: MessageRef,

    /// Custom regex; its first capture group is the code
    #[arg(long)]
    pub regex: Option<String>,

    /// Wait for a new message containing a code instead of reading one
    #[arg(long, short, conflicts_with_all = ["id", "recent"])]
    pub wait: bool,

    #[command(flatten)]
    pub filter: WaitFilter,
}

#[derive(Debug, Args)]
pub struct LinksArgs {
    #[command(flatten)]
    pub msg: MessageRef,

    /// Only links matching this regex
    #[arg(long, short)]
    pub r#match: Option<String>,

    /// Print only the best link
    #[arg(long)]
    pub first: bool,

    /// Open the best link in the browser
    #[arg(long)]
    pub open: bool,
}

#[derive(Debug, Args)]
pub struct WatchArgs {
    /// Watch every stored inbox, not just the active one
    #[arg(long, short = 'a')]
    pub all_inboxes: bool,

    /// Desktop notification for each new message
    #[arg(long, short)]
    pub notify: bool,

    /// Seconds between checks [default: 5]
    #[arg(long)]
    pub interval: Option<u64>,
}

#[derive(Debug, Args)]
pub struct SaveArgs {
    /// Message id or list number
    pub id: String,

    /// Attachment ids or file names (all when omitted)
    pub attachments: Vec<String>,

    /// Target directory
    #[arg(long, short, default_value = ".")]
    pub output: std::path::PathBuf,
}

#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// Message id or list number
    #[arg(required_unless_present = "all")]
    pub id: Option<String>,

    /// Delete every message in the inbox
    #[arg(long, conflicts_with = "id")]
    pub all: bool,
}

#[derive(Debug, Args)]
pub struct ProviderArg {
    /// Provider to ask (default: first in your provider order)
    #[arg(long, short)]
    pub provider: Option<String>,
}

#[derive(Debug, Args)]
pub struct InboxesArgs {
    /// Forget inboxes that have expired
    #[arg(long)]
    pub prune: bool,
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Full address of the existing account
    pub address: String,

    /// Account password (or TMPMAIL_PASSWORD)
    #[arg(long, env = "TMPMAIL_PASSWORD", hide_env_values = true)]
    pub password: Option<String>,

    /// mailtm (default), mailgw or maildrop
    #[arg(long, short)]
    pub provider: Option<String>,

    /// Label to find this inbox later (e.g. github)
    #[arg(long, short)]
    pub label: Option<String>,
}

#[derive(Debug, Args)]
pub struct DropArgs {
    /// Inbox to drop (default: the active one)
    pub address: Option<String>,

    /// Only forget it locally; leave it at the provider
    #[arg(long)]
    pub local: bool,
}

#[derive(Debug, Args)]
pub struct RotateArgs {
    /// Keep the old inbox instead of dropping it
    #[arg(long)]
    pub keep: bool,

    /// Copy the new address to the clipboard
    #[arg(long, short)]
    pub copy: bool,
}

#[derive(Debug, Args)]
pub struct ProvidersArgs {
    /// Check whether each provider is reachable right now
    #[arg(long)]
    pub check: bool,
}

/// Whether `name` is a subcommand or one of its aliases.
fn is_subcommand(name: &str) -> bool {
    use clap::CommandFactory;
    name == "help"
        || Cli::command()
            .get_subcommands()
            .any(|c| c.get_name() == name || c.get_all_aliases().any(|a| a == name))
}

/// Global options that take a value, so their value is not mistaken for a
/// command when scanning legacy arguments.
const VALUE_GLOBALS: &[&str] = &["-i", "--inbox", "-b", "--browser", "--clipboard-cmd"];

/// Translates the original tmpmail interface (`tmpmail`, `tmpmail -g [ADDR]`,
/// `tmpmail -r`, `tmpmail ID`, ...) into subcommands, leaving everything else
/// untouched.
pub fn rewrite_legacy(args: Vec<String>) -> Vec<String> {
    let mut out = vec![args.first().cloned().unwrap_or_else(|| "tmpmail".into())];
    let mut rest = args.into_iter().skip(1).peekable();
    let mut globals = Vec::new();
    let mut command: Option<Vec<String>> = None;

    while let Some(arg) = rest.next() {
        if let Some(cmd) = &mut command {
            cmd.push(arg);
            continue;
        }
        let a = arg.as_str();
        if VALUE_GLOBALS.contains(&a) {
            globals.push(arg);
            if let Some(v) = rest.next() {
                globals.push(v);
            }
            continue;
        }
        if a.starts_with("--") && a.contains('=') {
            globals.push(arg);
            continue;
        }
        command = Some(match a {
            "-g" | "--generate" => {
                let mut c = vec!["new".to_string()];
                if let Some(addr) = rest.next_if(|n| !n.starts_with('-')) {
                    c.push(addr);
                }
                c
            }
            "-r" | "--recent" => vec!["read".into(), "--recent".into()],
            "-d" | "--domains" => vec!["domains".into()],
            "-c" | "--copy" => vec!["copy".into()],
            "-t" | "--text" => vec!["read".into()],
            "--json" | "--no-color" | "-q" | "--quiet" => {
                globals.push(arg);
                continue;
            }
            _ if a.starts_with('-') => vec![arg],
            _ if is_subcommand(a) => vec![arg],
            _ => vec!["read".into(), arg],
        });
    }

    let command = command.unwrap_or_else(|| vec!["ls".into()]);
    out.extend(globals);
    out.extend(command);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rw(s: &str) -> String {
        let args = std::iter::once("tmpmail")
            .chain(s.split_whitespace())
            .map(String::from)
            .collect();
        rewrite_legacy(args)[1..].join(" ")
    }

    #[test]
    fn legacy_flags_map_to_subcommands() {
        assert_eq!(rw(""), "ls");
        assert_eq!(rw("-g"), "new");
        assert_eq!(
            rw("--generate otter@maxxspace.com"),
            "new otter@maxxspace.com"
        );
        assert_eq!(rw("-g --copy"), "new --copy");
        assert_eq!(rw("-r"), "read --recent");
        assert_eq!(rw("-t 42"), "read 42");
        assert_eq!(rw("42"), "read 42");
        assert_eq!(rw("-d"), "domains");
        assert_eq!(rw("-c"), "copy");
        assert_eq!(rw("-b w3m -r"), "-b w3m read --recent");
        assert_eq!(rw("--json"), "--json ls");
    }

    #[test]
    fn modern_usage_is_untouched() {
        assert_eq!(rw("new --provider dropmail"), "new --provider dropmail");
        assert_eq!(rw("--json wait --otp"), "--json wait --otp");
        assert_eq!(rw("-i github ls"), "-i github ls");
        assert_eq!(rw("--help"), "--help");
        assert_eq!(rw("--version"), "--version");
        assert_eq!(rw("read 3 --json"), "read 3 --json");
    }

    #[test]
    fn cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
