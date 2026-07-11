mod crds;
mod error;
mod owner;
mod project;
mod sandbox;

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
    tracing::info!("controller starting (owner + project reconcilers active)");

    let client = kube::Client::try_default()
        .await
        .expect("failed to build kube client from in-cluster or kubeconfig context");

    let sandbox_image = std::env::var("SANDBOX_IMAGE")
        .unwrap_or_else(|_| "vanyline-sandbox:latest".to_string());

    let owner_ctx = Arc::new(owner::Context { client: client.clone() });
    let project_ctx = Arc::new(project::Context {
        client: client.clone(),
        sandbox_image,
    });

    let owner_run = owner::build_controller(client.clone())
        .run(owner::reconcile, owner::error_policy, owner_ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                tracing::warn!(error = %e, "owner reconcile loop error");
            }
        });

    let project_run = project::build_controller(client)
        .run(project::reconcile, project::error_policy, project_ctx)
        .for_each(|res| async move {
            if let Err(e) = res {
                tracing::warn!(error = %e, "project reconcile loop error");
            }
        });

    tokio::join!(owner_run, project_run);
}