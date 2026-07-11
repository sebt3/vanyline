mod crds;
mod error;
mod owner;
mod project;

use clap::Parser;
use futures::StreamExt;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "vanyline-controller")]
struct Cli {
    /// Print CRD manifests and exit
    #[arg(long)]
    crds: bool,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    if cli.crds {
        print!("{}", crds::crd_manifests());
        return;
    }

    let _ = tracing_subscriber::fmt::try_init();
    tracing::info!("controller starting (owner reconciler active)");

    let client = kube::Client::try_default()
        .await
        .expect("failed to build kube client from in-cluster or kubeconfig context");
    let ctx = Arc::new(owner::Context { client: client.clone() });

    owner::build_controller(client)
        .run(owner::reconcile, owner::error_policy, ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                tracing::warn!(error = %e, "owner reconcile loop error");
            }
        })
        .await;
}