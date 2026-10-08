use clap::Subcommand;

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
pub enum Commands {
    /// List K8s Projects
    List,
    /// Show a single Project
    Show { name: String },
    /// Create a Project
    Create {
        name: String,
        #[arg(long = "repo-url")]
        repo_url: String,
        #[arg(long)]
        default_branch: Option<String>,
        #[arg(long)]
        storage_size: Option<String>,
        #[arg(long)]
        storage_class: Option<String>,
        #[arg(long)]
        storage_access_mode: Option<String>,
        #[arg(long)]
        git_secret: Option<String>,
        /// Répétable : --cache cargo --cache pnpm
        #[arg(long = "cache")]
        caches: Vec<String>,
        #[arg(long)]
        fetch_interval: Option<String>,
    },
    /// Delete a Project
    Delete { name: String },
}
