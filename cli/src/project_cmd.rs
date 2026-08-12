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
        #[arg(long)]
        owner: String,
        #[arg(long = "repo-url")]
        repo_url: String,
        #[arg(long)]
        default_branch: Option<String>,
        /// Nom du PVC existant (cas kydah-code). Nécessite --existing-pvc-name pour
        /// que --existing-pvc-subpath ait un effet.
        #[arg(long)]
        existing_pvc_name: Option<String>,
        #[arg(long)]
        existing_pvc_subpath: Option<String>,
        #[arg(long)]
        storage_size: Option<String>,
        #[arg(long)]
        storage_class: Option<String>,
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
