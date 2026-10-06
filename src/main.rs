mod app;
mod cli;
mod config;
mod error;
mod extract;
mod http;
mod provider;
mod render;
mod store;
mod system;

use clap::{CommandFactory, Parser};

use crate::cli::{Cli, Command};
use crate::error::Error;

fn main() {
    // Exit quietly when piped into `head` and the like, instead of panicking
    // on a broken pipe.
    #[cfg(unix)]
    // SAFETY: called once at startup before any threads exist.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }

    let args = cli::rewrite_legacy(std::env::args().collect());
    // Clap would exit with 2 on bad usage, which is our "wait timed out" code.
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            std::process::exit(if e.use_stderr() { 64 } else { 0 });
        }
    };

    if let Command::Completions { shell } = cli.command {
        clap_complete::generate(
            shell,
            &mut Cli::command(),
            "tmpmail",
            &mut std::io::stdout(),
        );
        return;
    }

    let json = cli.global.json;
    let result = app::App::new(cli.global).and_then(|mut app| app.run(cli.command));
    if let Err(e) = result {
        report(&e, json);
        std::process::exit(e.exit_code());
    }
}

fn report(e: &Error, json: bool) {
    if json {
        let v = serde_json::json!({ "error": { "kind": e.kind(), "message": e.to_string() } });
        println!("{v}");
    } else {
        let mut msg = e.to_string();
        if let Error::Other(inner) = e {
            // anyhow keeps the cause chain; show it.
            msg = format!("{inner:#}");
        }
        eprintln!("tmpmail: {}", render::clean_line(&msg));
    }
}
