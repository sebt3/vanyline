mod crds;

use std::process;

use clap::Parser;

#[derive(Parser)]
#[command(name = "vanyline-controller")]
struct Cli {
    /// Print CRD manifests and exit
    #[arg(long)]
    crds: bool,
}

fn main() {
    let cli = Cli::parse();

    if cli.crds {
        print!("{}", crds::crd_manifests());
        return;
    }

    // Minimal tracing setup (tracing-subscriber is a dep but no runtime yet)
    let _ = tracing_subscriber::fmt::try_init();
    tracing::info!("controller starting (reconcilers: not yet implemented)");
    process::exit(0);
}