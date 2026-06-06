use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use ttrack_pro::audit;
use ttrack_pro::config::Config;
use ttrack_pro::record::{self, RecordOptions};
use ttrack_pro::store;

#[derive(Parser)]
#[command(name = "ttrack")]
#[command(about = "Linux terminal session tracker - Rust pro implementation")]
struct Cli {
    #[arg(short = 'c', long = "check", global = true)]
    check: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(alias = "record")]
    Rec {
        #[arg(short = 'o')]
        out: Option<PathBuf>,
        #[arg(short = 'q')]
        quiet: bool,
        #[arg(trailing_var_arg = true)]
        cmd: Vec<String>,
    },
    Play {
        #[arg(long, default_value_t = 1.0)]
        speed: f64,
        target: String,
    },
    #[command(alias = "list")]
    Ls {
        #[arg(long, short = 'a')]
        all: bool,
        #[arg(long)]
        user: Option<String>,
    },
    Tail {
        #[arg(short = 'f')]
        follow: bool,
        #[arg(short = 'n', default_value_t = 20)]
        lines: usize,
        id: String,
    },
    Tree,
    Search {
        #[arg(short = 'i')]
        insensitive: bool,
        #[arg(long)]
        user: Option<String>,
        pattern: String,
    },
    Export {
        #[arg(short = 'o')]
        out: Option<PathBuf>,
        id: String,
    },
    Prune {
        #[arg(long)]
        yes: bool,
    },
    Version,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::load();

    if cli.check {
        println!("socket_path={}", cfg.socket_path.display());
        println!("central_dir={}", cfg.central_dir.display());
        println!("key_file={}", cfg.key_file.display());
        println!("local_dir={}", cfg.local_dir.display());
        println!("session_cap={}", cfg.session_cap);
        return Ok(());
    }

    match cli.command {
        Some(Commands::Rec { out, quiet, cmd }) => record::run(&cfg, RecordOptions { out, quiet, cmd }),
        Some(Commands::Play { speed, target }) => audit::play(&cfg, &target, speed),
        Some(Commands::Ls { all, user }) => match (all, user) {
            (true, _) => audit::ls_all(&cfg),
            (false, Some(user)) => audit::ls_user(&cfg, &user),
            (false, None) => store::list_local(&cfg),
        },
        Some(Commands::Tail { follow, lines, id }) => {
            if follow {
                audit::tail_live(&cfg, &id)
            } else {
                audit::tail_static(&cfg, &id, lines)
            }
        }
        Some(Commands::Tree) => audit::tree(&cfg),
        Some(Commands::Search { insensitive, user, pattern }) => audit::search(&cfg, &pattern, user, insensitive),
        Some(Commands::Export { out, id }) => audit::export(&cfg, &id, out),
        Some(Commands::Prune { yes }) => audit::prune(&cfg, yes),
        Some(Commands::Version) => {
            println!("ttrack-pro {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        None => {
            eprintln!("run 'ttrack --help' for usage");
            Ok(())
        }
    }
}
