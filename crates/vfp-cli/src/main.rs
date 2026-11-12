//! `vfp` command line. Same commands as the Python version: init, receivers, asset, mark, trace.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use vfp_core::pipeline::{self, PlanOptions};
use vfp_core::workspace::Workspace;
use vfp_core::{Error, Result};

#[derive(Parser)]
#[command(name = "vfp", version, about = "Per-receiver invisible watermarking and leak tracing")]
struct Cli {
    /// Workspace folder (default: $VFP_HOME or ./.vfp)
    #[arg(long, global = true, env = "VFP_HOME", default_value = ".vfp")]
    home: PathBuf,
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

fn run(cli: Cli) -> Result<u8> {
    let home = cli.home;
    match cli.cmd {
        Cmd::Init => {
            let existed = home.join("master.key").exists();
            Workspace::init(&home)?;
            let abs = std::path::absolute(&home).unwrap_or(home.clone());
            println!("Workspace ready: {}", abs.display());
            if !existed {
                println!("A new secret key was made: {}", abs.join("master.key").display());
            }
            println!("BACK UP master.key. Without it you cannot trace a leak. Never share it.");
        }
        Cmd::Receivers { action, names, note } => {
            let ws = Workspace::open(&home)?;
            match action {
                RecAction::Add => {
                    for n in &names {
                        ws.add_receiver(n, note.as_deref().unwrap_or(""))?;
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
                        ws.add_receiver(name.trim(), note.trim())?;
                        println!("added receiver: {}", name.trim());
                    }
                }
                RecAction::List => {
                    for r in ws.receivers()? {
                        println!("{:>4}  {:<30} {}", r.id, r.name, r.note);
                    }
                }
            }
        }
        Cmd::Asset { action, file, name, max_receivers, colluders, eps, strength, slots, crf } => {
            let opts = PlanOptions { max_receivers, eps, strength, slots, n_colluders: colluders, crf };
            let need_file = || file.as_deref().ok_or_else(|| Error("asset add/plan needs a file".into()));
            match action {
                AssetAction::Add => {
                    let ws = Workspace::open(&home)?;
                    let (a, plan) = pipeline::add_asset(&ws, need_file()?, name.as_deref(), &opts)?;
                    println!("registered asset '{}' (id {}, {})", a.name, a.id, a.kind);
                    print_plan(&plan, a.max_receivers, a.eps);
                }
                AssetAction::Plan => {
                    let plan = pipeline::plan_file(need_file()?, &opts)?;
                    print_plan(&plan, max_receivers, eps);
                }
                AssetAction::List => {
                    let ws = Workspace::open(&home)?;
                    for a in ws.assets()? {
                        let n = ws.issues(a.id)?.len();
                        println!("{:>4}  {:<30} {:<6} symbols={:<6} copies issued={}", a.id, a.name, a.kind, a.m, n);
                    }
                }
            }
        }
        Cmd::Mark { asset, to, all, out } => {
            let ws = Workspace::open(&home)?;
            let a = ws.asset(&asset)?;
            let targets: Vec<String> = if all { ws.receivers()?.into_iter().map(|r| r.name).collect() } else { to };
            if targets.is_empty() {
                eprintln!("Say who gets a copy: --to NAME [NAME ...] or --all");
                return Ok(2);
            }
            for name in targets {
                println!("marking for {name} ...");
                let show = |n: usize, total: usize| eprint!("\r  frame {n}/{total}");
                let progress: Option<&dyn Fn(usize, usize)> = if a.kind == "video" { Some(&show) } else { None };
                let p = pipeline::mark_for_receiver(&ws, &a, &name, &out, progress)?;
                if a.kind == "video" {
                    eprintln!();
                }
                println!("  -> {}", p.display());
            }
        }
        Cmd::Trace { file, asset, top, json } => {
            let ws = Workspace::open(&home)?;
            let r = pipeline::trace_leak(&ws, &file, asset.as_deref())?;
            if json {
                println!("{}", serde_json::to_string_pretty(&r).map_err(Error::from)?);
                return Ok(0);
            }
            println!("asset: {} ({})   accusation threshold: {:.1}", r.asset, r.kind, r.threshold);
            println!("evidence: {}", r.evidence);
            for w in &r.warnings {
                println!("WARNING: {w}");
            }
            println!("\n{:>4}  {:<30}{:>9}  ", "rank", "receiver", "score");
            for (i, (n, s)) in r.ranking.iter().take(top).enumerate() {
                let mark = if *s > r.threshold { "<-- above threshold" } else { "" };
                println!("{:>4}  {:<30}{:>9.1}  {}", i + 1, n, s, mark);
            }
            println!("\n{}", r.verdict);
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
