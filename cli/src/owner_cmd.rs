use clap::Subcommand;

#[derive(Subcommand)]
pub enum Commands {
    /// List K8s Owners
    List,
    /// Show a single Owner
    Show { name: String },
    /// Create an Owner
    Create {
        name: String,
        /// PVC home existant (ex. celui de code-server). Omis: PVC créé par le controller.
        #[arg(long)]
        existing_pvc: Option<String>,
        #[arg(long)]
        home_size: Option<String>,
        #[arg(long)]
        home_storage_class: Option<String>,
        /// Taille par défaut appliquée aux futurs Projects de cet Owner.
        #[arg(long)]
        project_default_storage_size: Option<String>,
        #[arg(long)]
        project_default_storage_class: Option<String>,
    },
    /// Delete an Owner
    Delete { name: String },
}