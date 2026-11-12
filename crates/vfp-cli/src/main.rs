//! `vfp` command line. Same commands as the Python version: init, receivers, asset, mark, trace.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use vfp_core::backend::{check_url, Backend, LocalBackend, RemoteBackend, SealedBackend, ServerLink};
use vfp_core::pipeline::{self, PlanOptions};
use vfp_core::strength;
use vfp_core::vault::{Dek, KdfParams, Protection, VaultFile, VAULT_FILE};
use vfp_core::workspace::{Protection as WsProtection, Workspace};
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
    /// Create a workspace. It is encrypted with a passphrase unless you say --plain.
    Init {
        /// No encryption (not for sensitive files)
        #[arg(long)]
        plain: bool,
    },
    /// Passphrase and encryption of the workspace (or of the connected server)
    Vault {
        #[command(subcommand)]
        action: VaultCmd,
    },
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

#[derive(Subcommand)]
enum VaultCmd {
    /// Encrypt this workspace, or create the vault on the connected server
    Init {
        /// On a server: zero-knowledge mode. The server never gets your key and cannot read your files.
        /// Without this flag the server is "protected": it gets the key per request over HTTPS.
        #[arg(long)]
        zero_knowledge: bool,
    },
    /// Say whether the workspace is encrypted
    Status,
    /// Change the passphrase of this workspace. Files are not touched.
    Passwd,
    /// Print a new strong passphrase (7 random words)
    Generate,
}

/// Read a passphrase from the file in VFP_PASSPHRASE_FILE, or ask at the terminal (hidden).
/// It is never a command-line argument.
fn passphrase(prompt: &str) -> Result<String> {
    if let Ok(p) = std::env::var("VFP_PASSPHRASE_FILE") {
        let t = std::fs::read_to_string(&p).map_err(|e| Error(format!("cannot read {p}: {e}")))?;
        return Ok(t.trim_end_matches(['\r', '\n']).to_string());
    }
    rpassword::prompt_password(prompt).map_err(|e| Error(format!("cannot read the passphrase ({e}). Set VFP_PASSPHRASE_FILE.")))
}

/// Ask for a NEW passphrase twice. It must be strong.
fn new_passphrase() -> Result<String> {
    let a = passphrase("New passphrase (or run `vfp vault generate`): ")?;
    let s = strength::assess(&a);
    if !s.ok {
        return Err(Error(format!("Passphrase is too weak ({:.0} bits, need {:.0}). {}", s.bits, strength::MIN_BITS, s.advice.join(" "))));
    }
    if std::env::var("VFP_PASSPHRASE_FILE").is_err() && passphrase("Again: ")? != a {
        return Err(Error("the two passphrases differ".into()));
    }
    Ok(a)
}

fn backup_notice(path: &Path) {
    println!("BACK UP {}. It holds your wrapped key. Without the passphrase the data cannot be opened. There is no recovery.", path.display());
}

/// Unlock a local workspace if it has a vault.
fn unlock_local(home: &Path) -> Result<Option<Dek>> {
    if Workspace::protection(home) != WsProtection::Vault {
        return Ok(None);
    }
    let v = VaultFile::load(&home.join(VAULT_FILE))?.ok_or_else(|| Error("vault.json is missing".into()))?;
    Ok(Some(v.unlock(&passphrase("Passphrase: ")?)?))
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
            return remote_unlocked(&cli.home, url, &token);
        }
        if let Some(l) = ServerLink::load(&cli.home)? {
            return remote_unlocked(&cli.home, &l.url, &l.token);
        }
    }
    Ok(Box::new(LocalBackend::with_key(&cli.home, unlock_local(&cli.home)?)))
}

/// Connect to a server. If it has a vault, ask for the passphrase and unlock it here (the server never sees it).
fn remote_unlocked(home: &Path, url: &str, token: &str) -> Result<Box<dyn Backend>> {
    let plain = RemoteBackend::new(url, token);
    match plain.vault()? {
        None => Ok(Box::new(plain)),
        Some(v) => {
            let dek = v.unlock(&passphrase("Passphrase: ")?)?;
            if v.protection == Protection::ZeroKnowledge {
                // zero-knowledge: the key stays here, this device does the work
                Ok(Box::new(SealedBackend::open(url, token, dek, SealedBackend::cache_dir(home, url))))
            } else {
                Ok(Box::new(RemoteBackend::new(url, token).with_key(Some(dek))?))
            }
        }
    }
}

fn run(cli: Cli) -> Result<u8> {
    let home = cli.home.clone();
    match &cli.cmd {
        Cmd::Init { plain } => {
            let abs = std::path::absolute(&home).unwrap_or(home.clone());
            if *plain {
                let existed = home.join("master.key").exists();
                Workspace::init(&home)?;
                println!("Workspace ready: {}", abs.display());
                if !existed {
                    println!("A new secret key was made: {}", abs.join("master.key").display());
                }
                println!("WARNING: nothing here is encrypted. Run `vfp vault init` to encrypt it.");
                println!("BACK UP master.key. Without it you cannot trace a leak. Never share it.");
            } else if home.join("vfp.db").exists() {
                return Err(Error("a workspace already exists here. To encrypt it run: vfp vault init".into()));
            } else {
                let pass = new_passphrase()?;
                let (vf, dek) = VaultFile::create(&pass, Protection::Protected, KdfParams::DESKTOP)?;
                Workspace::init_encrypted(&home, &vf, &dek)?;
                println!("Encrypted workspace ready: {}", abs.display());
                backup_notice(&abs.join(VAULT_FILE));
            }
            return Ok(0);
        }
        Cmd::Vault { action } => {
            match action {
                VaultCmd::Generate => {
                    let p = strength::generate();
                    println!("{p}");
                    eprintln!("({:.0} bits). Write it down and keep it safe. There is no recovery.", strength::assess(&p).bits);
                }
                VaultCmd::Status => {
                    let abs = std::path::absolute(&home).unwrap_or(home.clone());
                    match (Workspace::protection(&home), home.join("vfp.db").exists()) {
                        (WsProtection::Vault, _) => println!("{}: encrypted (vault.json present)", abs.display()),
                        (_, true) => println!("{}: NOT encrypted. Run: vfp vault init", abs.display()),
                        _ => println!("{}: no workspace here", abs.display()),
                    }
                    if let Some(l) = ServerLink::load(&home)? {
                        let info = RemoteBackend::new(&l.url, &l.token).info()?;
                        println!("server {}: {}", l.url, info["protection"].as_str().unwrap_or("?"));
                    }
                }
                VaultCmd::Init { zero_knowledge } => {
                    if !cli.local {
                        if let Some(l) = ServerLink::load(&home)? {
                            let r = RemoteBackend::new(&l.url, &l.token);
                            if r.vault()?.is_some() {
                                return Err(Error("the server already has a vault".into()));
                            }
                            let pass = new_passphrase()?;
                            if *zero_knowledge {
                                let (vf, dek) = VaultFile::create(&pass, Protection::ZeroKnowledge, KdfParams::DESKTOP)?;
                                let cache = SealedBackend::cache_dir(&home, &l.url);
                                SealedBackend::create(&l.url, &l.token, &vf, dek, cache.clone())?;
                                println!("The zero-knowledge vault is ready. The server never saw your passphrase or key and cannot read your files.");
                                backup_notice(&cache.join(VAULT_FILE));
                                println!("The server holds a copy of this file too, but if the server is lost and you have no backup, the data is gone.");
                                return Ok(0);
                            }
                            let (vf, dek) = VaultFile::create(&pass, Protection::Protected, KdfParams::DESKTOP)?;
                            RemoteBackend::new(&l.url, &l.token).with_key(Some(dek))?.create_vault(&vf)?;
                            println!("The server vault is ready. The passphrase never left this device (the server holds only the key, per request, over HTTPS).");
                            return Ok(0);
                        }
                    }
                    if Workspace::protection(&home) == WsProtection::Vault {
                        return Err(Error("this workspace is already encrypted".into()));
                    }
                    if !home.join("vfp.db").exists() {
                        return Err(Error("no workspace here. Run: vfp init".into()));
                    }
                    let pass = new_passphrase()?;
                    let (vf, dek) = VaultFile::create(&pass, Protection::Protected, KdfParams::DESKTOP)?;
                    Workspace::migrate_to_vault(&home, &vf, &dek)?;
                    println!("Encrypted. Originals and master.key are now sealed; the plain ones were removed.");
                    backup_notice(&home.join(VAULT_FILE));
                }
                VaultCmd::Passwd if !cli.local && ServerLink::load(&home)?.is_some() => {
                    let l = ServerLink::load(&home)?.ok_or_else(|| Error("not connected".into()))?;
                    let r = RemoteBackend::new(&l.url, &l.token);
                    let v = r.vault()?.ok_or_else(|| Error("the server has no vault".into()))?;
                    if v.protection != Protection::ZeroKnowledge {
                        return Err(Error("changing the passphrase on a protected server is not supported yet. Use a local workspace or a zero-knowledge server.".into()));
                    }
                    let old = passphrase("Current passphrase: ")?;
                    v.unlock(&old)?;
                    let new = new_passphrase()?;
                    let nv = v.change_passphrase(&old, &new)?;
                    r.replace_vault(&nv)?;
                    let cache = SealedBackend::cache_dir(&home, &l.url);
                    if cache.exists() {
                        nv.save(&cache.join(VAULT_FILE))?;
                    }
                    println!("Passphrase changed on the server. Files were not touched.");
                    println!("Other devices use the new passphrase from now on.");
                }
                VaultCmd::Passwd => {
                    let path = home.join(VAULT_FILE);
                    let v = VaultFile::load(&path)?.ok_or_else(|| Error("this workspace is not encrypted".into()))?;
                    let old = passphrase("Current passphrase: ")?;
                    v.unlock(&old)?;
                    let new = new_passphrase()?;
                    v.change_passphrase(&old, &new)?.save(&path)?;
                    println!("Passphrase changed. Files were not touched.");
                    backup_notice(&path);
                }
            }
            return Ok(0);
        }
        Cmd::Connect { url, token } => {
            check_url(url)?;
            let remote = RemoteBackend::new(url, token);
            let info = remote.info()?;
            if info["protection"] == "uninitialized" {
                println!("This server has no vault yet. Run: vfp vault init (add --zero-knowledge so the server can never read your files)");
            }
            ServerLink { url: url.clone(), token: token.clone() }.save(&home)?;
            println!("Connected to {url} (role: {}, protection: {}).", info["role"].as_str().unwrap_or("?"), info["protection"].as_str().unwrap_or("?"));
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
        Cmd::Init { .. } | Cmd::Vault { .. } | Cmd::Connect { .. } | Cmd::Disconnect => unreachable!(),
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
