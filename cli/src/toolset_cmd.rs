use clap::Subcommand;

#[derive(Subcommand)]
pub enum Commands {
    /// List configured toolsets
    List,
}
