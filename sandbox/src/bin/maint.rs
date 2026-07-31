//! `vanyline-maint` — utilitaire de maintenance des workspaces (Jobs controller).

use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

use vanyline_sandbox::maint;

#[derive(Parser)]
#[command(name = "vanyline-maint", version)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Crée les répertoires de cache et clone bare le repo (idempotent).
    Init {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        workspace: PathBuf,
        /// Répétable : --cache cargo --cache pnpm
        #[arg(long = "cache")]
        caches: Vec<String>,
    },
    /// `git fetch --prune` sur le clone bare.
    Fetch {
        #[arg(long)]
        workspace: PathBuf,
    },
    /// Supprime repo.git, worktrees et cache du workspace.
    Purge {
        #[arg(long)]
        workspace: PathBuf,
    },
    /// Crée le worktree de la branche pour une sandbox (idempotent).
    Checkout {
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        sandbox: String,
        #[arg(long)]
        branch: String,
        #[arg(long)]
        default_branch: Option<String>,
    },
    /// Retire le worktree d'une sandbox (worktree remove + prune).
    Remove {
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long)]
        sandbox: String,
    },
    /// Détection des langages du workspace (stub — implémenté par WS-10).
    Detect {
        #[arg(long)]
        workspace: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Cmd::Init {
            repo,
            workspace,
            caches,
        } => maint::validate_repo(&repo).and_then(|()| maint::run_init(&workspace, &repo, &caches)),
        Cmd::Fetch { workspace } => maint::run_fetch(&workspace),
        Cmd::Purge { workspace } => maint::run_purge(&workspace),
        Cmd::Checkout {
            workspace,
            sandbox,
            branch,
            default_branch,
        } => maint::run_checkout(&workspace, &sandbox, &branch, default_branch.as_deref()),
        Cmd::Remove { workspace, sandbox } => maint::run_remove(&workspace, &sandbox),
        Cmd::Detect { workspace } => maint::run_detect(&workspace).map(|json| {
            println!("{json}");
        }),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("vanyline-maint: {e}");
            ExitCode::FAILURE
        }
    }
}
