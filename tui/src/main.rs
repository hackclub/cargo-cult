use std::process::{exit, Stdio};
use std::sync::Arc;
use clap::{Parser, Subcommand};
use dirs::home_dir;

use dotenv::dotenv;
use glob::glob;
use russh::Pty;
use tokio::process::Command;
use tokio::sync::Mutex;
use crate::database::SubmissionsAirtableBase;

use crate::ssh_server::ssh_server;
use crate::terminal::{make_terminal_app};

mod database;
mod app;
mod ssh_client;
mod ssh_server;
mod terminal;

#[tokio::main]
async fn main() {
    dotenv().ok();

    let args = Cli::parse();
    let action = match args.command {
        SubCommand::CargoCult { command } => command,
        SubCommand::Action(action) => action
    };

    match action {
        Action::Ssh => {
            ssh_server().await
        }
        Action::InstallAllPackages => {
            let mut airtable = SubmissionsAirtableBase::new();
            let packages: Vec<String> = airtable.get().await.unwrap().iter().map(|entry| entry.package_name.clone().unwrap()).collect();

            Command::new("cargo")
                .arg("install")
                .args(packages)
                .spawn().expect("TODO").wait().await.unwrap();
        }
        Action::SSHEntrypoint { package_name, author, username} => {
            println!("Welcome! Run '{package_name}' to test out {author}'s CLI! Or, run 'readme {package_name}' to view the readme.");
            println!("This Ubuntu VM will self-destruct in 30 minutes. Run 'exit' to exit.");
            println!("psst: all the other projects are installed here, so feel free to try them out.");
            Command::new("bash")
                .env("PS1", format!("{}@cargo-cult:\\w\\$ ", username))
                .arg("--noprofile").arg("--norc")
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn().expect("TODO").wait().await.unwrap();
        }
        Action::Readme { package_name } => {
            let Some(Ok(path)) = glob(
                format!("{}/.cargo/registry/src/*/{}*/README.md", home_dir().unwrap().display(), package_name).as_str()
            ).unwrap().next() else {
                eprintln!("Could not find package README!");
                exit(1);
            };

            Command::new("glow")
                .arg(path.display().to_string())
                .arg("-p")
                .stdin(Stdio::inherit())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .spawn().expect("TODO").wait().await.unwrap();
        }
        _ => {
            let mut app = make_terminal_app().await;
            app.run().await.unwrap();
        }
    }
}

#[derive(Debug, Parser)]
#[clap(multicall = true)]
struct Cli {
    #[command(subcommand)]
    command: SubCommand
}

#[derive(Debug, Subcommand)]
enum SubCommand {
    #[command(flatten)]
    Action(Action),
    
    CargoCult {
        #[command(subcommand)]
        command: Action
    }
}

#[derive(Debug, Subcommand)]
enum Action {
    Ssh,

    Menu,
    Gallery,

    #[command(hide = true)]
    InstallAllPackages,
    #[command(hide = true)]
    SSHEntrypoint {
        #[arg(index = 1)]
        username: String,
        #[arg(index = 2)]
        package_name: String,
        #[arg(index = 3)]
        author: String
    },
    #[command(hide = true)]
    Readme {
        #[arg(index = 1)]
        package_name: String
    }
}

#[derive(Clone)]
struct TerminalParams {
    term: String,
    col_width: u32,
    row_height: u32,
    modes: Vec<(Pty, u32)>,
    username: String
}

type SharedTerminalParams = Arc<Mutex<TerminalParams>>;

#[derive(Clone)]
struct TerminalCode {
    ascii_code: Option<AsciiCode>,
    raw_bytes: Vec<u8>
}

#[derive(PartialEq, Clone)]
enum AsciiCode {
    Char(u8),
    Backspace,
    Enter,
    ArrowDown,
    ArrowUp,
    EoT
}
