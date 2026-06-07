use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use ttrack_pro::ansible;
use ttrack_pro::audit;
use ttrack_pro::config::Config;
use ttrack_pro::crypto;
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
    Init,
    Search {
        #[arg(short = 'i')]
        insensitive: bool,
        #[arg(long)]
        user: Option<String>,
        #[arg(long, value_name = "YYYY-MM-DD")]
        from: Option<String>,
        #[arg(long, value_name = "YYYY-MM-DD")]
        to: Option<String>,
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
    Ansible {
        #[command(subcommand)]
        subcmd: AnsibleCmd,
    },
    #[command(hide = true, name = "ansible-ingest")]
    AnsibleIngest,
    #[command(hide = true, name = "__complete")]
    Complete {
        kind: String,
    },
}

#[derive(Subcommand)]
enum AnsibleCmd {
    List {
        #[arg(long)]
        user: Option<String>,
    },
    Show {
        #[arg(long)]
        user: Option<String>,
        run_id: String,
    },
    Incoming {
        #[arg(long)]
        user: Option<String>,
    },
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
        Some(Commands::Init) => {
            std::fs::create_dir_all(&cfg.central_dir).context("create central dir")?;
            let key = crypto::ensure_key(&cfg.key_file)?;
            let count = store::ingest_local(&cfg, &key)?;
            println!("key: {}", cfg.key_file.display());
            println!("central: {}", cfg.central_dir.display());
            if count > 0 {
                println!("ingested {count} local session(s)");
            }
            Ok(())
        }
        Some(Commands::Search { insensitive, user, from, to, pattern }) => {
            let from_ts = from.as_deref().map(parse_date).transpose()?;
            let to_ts = to.as_deref().map(parse_date).transpose()?;
            audit::search(&cfg, &pattern, user, insensitive, from_ts, to_ts)
        }
        Some(Commands::Export { out, id }) => audit::export(&cfg, &id, out),
        Some(Commands::Prune { yes }) => audit::prune(&cfg, yes),
        Some(Commands::Version) => {
            println!("ttrack-pro {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some(Commands::Ansible { subcmd }) => {
            let cur_user = store::current_username();
            match subcmd {
                AnsibleCmd::List { user } => {
                    let user = user.unwrap_or(cur_user);
                    ansible::cmd_list(&cfg, &user)
                }
                AnsibleCmd::Show { user, run_id } => {
                    let user = user.unwrap_or(cur_user);
                    ansible::cmd_show(&cfg, &user, &run_id)
                }
                AnsibleCmd::Incoming { user } => {
                    let user = user.unwrap_or(cur_user);
                    ansible::cmd_incoming(&cfg, &user)
                }
            }
        }
        Some(Commands::AnsibleIngest) => ansible::cmd_ingest(&cfg),
        Some(Commands::Complete { kind }) => {
            match kind.as_str() {
                "users" => {
                    for u in store::users(&cfg).unwrap_or_default() {
                        println!("{u}");
                    }
                }
                "central-sessions" => {
                    for user in store::users(&cfg).unwrap_or_default() {
                        for path in store::user_sessions(&cfg, &user).unwrap_or_default() {
                            if let Some(stem) = path.file_stem() {
                                println!("{}", stem.to_string_lossy());
                            }
                        }
                    }
                }
                "local-sessions" => {
                    let dir = cfg.local_dir.clone();
                    if let Ok(entries) = std::fs::read_dir(&dir) {
                        for entry in entries.flatten() {
                            let p = entry.path();
                            if p.extension().map(|x| x == "cast").unwrap_or(false) {
                                if let Some(stem) = p.file_stem() {
                                    println!("{}", stem.to_string_lossy());
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
            Ok(())
        }
        None => {
            eprintln!("run 'ttrack --help' for usage");
            Ok(())
        }
    }
}

fn parse_date(s: &str) -> Result<i64> {
    use chrono::NaiveDate;
    let d = NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .with_context(|| format!("invalid date '{s}' (expected YYYY-MM-DD)"))?;
    Ok(d.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp())
}
