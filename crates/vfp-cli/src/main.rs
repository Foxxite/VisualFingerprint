//! `vfp` command line. Same commands as the Python version: init, receivers, asset, mark, trace.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use vfp_core::backend::{check_url, Backend, LocalBackend, RemoteBackend, ServerLink};
use vfp_core::pipeline::{self, PlanOptions};
use vfp_core::workspace::Workspace;
use vfp_core::{Error, Result};

#[derive(Parser)]
#[command(name = "vfp", version, about = "Per-receiver invisible watermarking and leak tracing")]
struct Cli {
    /// Workspace folder (default: $VFP_HOME or ./.vfp)
    #[arg(long, global = true, env = "VFP_HOME", default_value = ".vfp")]
    home: PathBuf,
    /// Use a vfp-server instead of this device (default: the one saved by `vfp connect`)
    #[arg(long, global = true, env = "VFP_SERVER")]
    server: Option<String>,
    /// Token for the server
    #[arg(long, global = true, env = "VFP_TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// Ignore a saved server and use this device
    #[arg(long, global = true)]
    local: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, ValueEnum)]
enum RecAction {
    Add,
    Import,
    List,
}

#[derive(Clone, ValueEnum)]
enum AssetAction {
    Add,
    Plan,
    List,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a workspace and a secret key
    Init,
    /// Save a server address and token. Later commands then use that server.
    Connect {
        url: String,
        #[arg(long, env = "VFP_TOKEN", hide_env_values = true)]
        token: String,
    },
    /// Forget the saved server and go back to this device
    Disconnect,
    /// Add, import or list receivers
    Receivers {
        action: RecAction,
        /// Names to add, or a file for import (one 'name[,note]' per line)
        names: Vec<String>,
        #[arg(long)]
        note: Option<String>,
    },
    /// Register an original file, plan it, or list assets
    Asset {
        action: AssetAction,
        file: Option<PathBuf>,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, default_value_t = 1000)]
        max_receivers: u64,
        /// Design size of a colluder group
        #[arg(long, default_value_t = 3)]
        colluders: u64,
        /// False-accusation rate
        #[arg(long, default_value_t = 1e-3)]
        eps: f64,
        /// Mark strength
        #[arg(long)]
        strength: Option<f32>,
        /// Symbols per image, or per segment for audio and video
        #[arg(long)]
        slots: Option<usize>,
        /// Video only: x264 quality of marked copies (default 16; higher = smaller file)
        #[arg(long)]
        crf: Option<i32>,
    },
    /// Make marked copies for receivers
    Mark {
        asset: String,
        #[arg(long, num_args = 0..)]
        to: Vec<String>,
        #[arg(long)]
        all: bool,
        #[arg(long, default_value = "marked")]
        out: PathBuf,
    },
    /// Find who leaked a file
    Trace {
        file: PathBuf,
        /// Asset name or id (default: try all of the same type)
        #[arg(long)]
        asset: Option<String>,
        #[arg(long, default_value_t = 5)]
        top: usize,
        #[arg(long)]
        json: bool,
    },
}

fn print_plan(p: &pipeline::Plan, max_receivers: u64, eps: f64) {
    println!("  code length: {} symbols", p.m);
    println!("  planned for up to {max_receivers} receivers, false-accusation rate {eps}");
    let g = p.guaranteed_colluders;
    let text = if g > 0 { g.to_string() } else { "none (single leaker only)".to_string() };
    println!("  guaranteed against colluder groups of up to: {text}");
    for n in &p.notes {
        println!("  NOTE: {n}");
    }
}

fn pick_backend(cli: &Cli) -> Result<Box<dyn Backend>> {
    if !cli.local {
        if let Some(url) = &cli.server {
            check_url(url)?;
            let token = cli.token.clone().ok_or_else(|| Error("a server needs a token: --token or VFP_TOKEN".into()))?;
            return Ok(Box::new(RemoteBackend::new(url, &token)));
        }
        if let Some(l) = ServerLink::load(&cli.home)? {
            return Ok(Box::new(RemoteBackend::new(&l.url, &l.token)));
        }
    }
    Ok(Box::new(LocalBackend::new(&cli.home)))
}

fn run(cli: Cli) -> Result<u8> {
    let home = cli.home.clone();
    match &cli.cmd {
        Cmd::Init => {
            let existed = home.join("master.key").exists();
            Workspace::init(&home)?;
            let abs = std::path::absolute(&home).unwrap_or(home.clone());
            println!("Workspace ready: {}", abs.display());
            if !existed {
                println!("A new secret key was made: {}", abs.join("master.key").display());
            }
            println!("BACK UP master.key. Without it you cannot trace a leak. Never share it.");
            return Ok(0);
        }
        Cmd::Connect { url, token } => {
            check_url(url)?;
            let remote = RemoteBackend::new(url, token);
            let info = remote.info()?;
            ServerLink { url: url.clone(), token: token.clone() }.save(&home)?;
            println!("Connected to {url} (role: {}, assets: {}).", info["role"].as_str().unwrap_or("?"), info["assets"]);
            println!("Later commands use this server. No original file is stored on this device.");
            return Ok(0);
        }
        Cmd::Disconnect => {
            let p = ServerLink::path(&home);
            if p.exists() {
                std::fs::remove_file(p)?;
            }
            println!("This device now uses its own workspace again.");
            return Ok(0);
        }
        Cmd::Asset { action: AssetAction::Plan, file, max_receivers, colluders, eps, strength, slots, crf, .. } => {
            let f = file.as_deref().ok_or_else(|| Error("asset plan needs a file".into()))?;
            let opts = PlanOptions { max_receivers: *max_receivers, eps: *eps, strength: *strength, slots: *slots, n_colluders: *colluders, crf: *crf };
            print_plan(&pipeline::plan_file(f, &opts)?, *max_receivers, *eps);
            return Ok(0);
        }
        _ => {}
    }

    let be = pick_backend(&cli)?;
    if !matches!(cli.cmd, Cmd::Trace { json: true, .. }) {
        eprintln!("[{}]", be.describe());
    }
    match cli.cmd {
        Cmd::Init | Cmd::Connect { .. } | Cmd::Disconnect => unreachable!(),
        Cmd::Receivers { action, names, note } => match action {
            RecAction::Add => {
                for n in &names {
                    be.add_receiver(n, note.as_deref().unwrap_or(""))?;
                    println!("added receiver: {n}");
                }
            }
            RecAction::Import => {
                let file = names.first().ok_or_else(|| Error("import needs a file".into()))?;
                for line in std::fs::read_to_string(file)?.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let (name, note) = line.split_once(',').unwrap_or((line, ""));
                    be.add_receiver(name.trim(), note.trim())?;
                    println!("added receiver: {}", name.trim());
                }
            }
            RecAction::List => {
                for r in be.receivers()? {
                    println!("{:>4}  {:<30} {}", r.id, r.name, r.note);
                }
            }
        },
        Cmd::Asset { action, file, name, max_receivers, colluders, eps, strength, slots, crf } => match action {
            AssetAction::Add => {
                let opts = PlanOptions { max_receivers, eps, strength, slots, n_colluders: colluders, crf };
                let f = file.as_deref().ok_or_else(|| Error("asset add needs a file".into()))?;
                let (a, plan) = be.add_asset(f, name.as_deref(), &opts)?;
                println!("registered asset '{}' (id {}, {})", a.name, a.id, a.kind);
                print_plan(&plan, a.max_receivers, eps);
            }
            AssetAction::List => {
                for a in be.assets()? {
                    println!("{:>4}  {:<30} {:<6} symbols={:<6} copies issued={}", a.id, a.name, a.kind, a.m, a.copies);
                }
            }
            AssetAction::Plan => unreachable!(),
        },
        Cmd::Mark { asset, to, all, out } => {
            let targets: Vec<String> = if all { be.receivers()?.into_iter().map(|r| r.name).collect() } else { to };
            if targets.is_empty() {
                eprintln!("Say who gets a copy: --to NAME [NAME ...] or --all");
                return Ok(2);
            }
            println!("marking for {} receiver(s) ...", targets.len());
            let show = |n: usize, total: usize| eprint!("\r  {n}/{total}");
            let copies = be.mark(&asset, &targets, &out, Some(&show))?;
            eprintln!();
            for (r, p) in copies {
                println!("{r}  -> {}", p.display());
            }
        }
        Cmd::Trace { file, asset, top, json } => {
            let r = be.trace(&file, asset.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r).map_err(Error::from)?);
                return Ok(0);
            }
            println!("asset: {} ({})   accusation threshold: {:.1}", r.asset, r.kind, r.threshold);
            println!("evidence: {}", r.evidence);
            for w in &r.warnings {
                println!("WARNING: {w}");
            }
            println!("
{:>4}  {:<30}{:>9}  ", "rank", "receiver", "score");
            for (i, (n, s)) in r.ranking.iter().take(top).enumerate() {
                let mark = if *s > r.threshold { "<-- above threshold" } else { "" };
                println!("{:>4}  {:<30}{:>9.1}  {}", i + 1, n, s, mark);
            }
            println!("
{}", r.verdict);
            println!("A score is statistical evidence, not proof. Keep the leaked file and this report.");
        }
    }
    Ok(0)
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

#[allow(dead_code)]
fn _unused(_: &Path) {}
